//! BacktestRunner：策略组编排入口（Phase 2 真撮合版）
//!
//! 全 Rust 原生回测编排：逐 bar 跑所有策略的 decide()，
//! 然后直接把 target_position 交给 BacktestEngine 撮合引擎撮合。
//!
//! 流程：
//! ```text
//! bars (时间升序)
//!   │
//!   ▼ 逐 bar 循环
//! ┌─────────────────────────────────────────┐
//! │ 1. engine.clock.set(bar.timestamp)       │
//! │ 2. 所有策略 → decide() → action→target   │
//! │    engine.set_target_position(inst, tgt) │
//! │ 3. engine.begin_bar_multi(legs)          │
//! │    → seed_liquidity + rebalance_to_target│
//! └─────────────────────────────────────────┘
//!   │
//!   ▼ 所有 bar 处理完
//! engine.run() → RunResult (聚合所有撮合统计)
//!   │
//!   ▼ 合并决策轨迹 + RunResult → BacktestResult
//! ```
//!
//! 设计约束：
//! - BacktestEngine 内部用 bar_id guard 保证同 bar rebalance 只触发一次
//! - auto_rebalance_threshold 默认 1e-6，避免目标无意义抖动也触发撮合
//! - RunResult 聚合所有 fill/order 统计 + NAV 曲线（bar_nav_curve 每 bar 一帧）

use std::collections::BTreeMap;
use std::sync::Arc;

use axon_backtest::L1MatchingEngine;
use axon_backtest::engine::{BacktestEngine, BacktestEngineConfig, FeeConfig};
use axon_core::market::bar::Bar;
use axon_core::{EventQueue, Instrument, SimulatedClock, Symbol, Timestamp};
use axon_strategy::trait_::{BarContext, Strategy, StrategyDecision};
use serde::Serialize;

/// 撮合自动 rebalance 阈值（避免目标无意义抖动也触发撮合）
const REBALANCE_THRESHOLD: f64 = 1e-6;

/// 回测运行器
///
/// 持有一个 BacktestEngine（L1 撮合 + SimulatedClock + EventQueue），
/// 逐 bar 编排 Strategy.decide() → set_target_position → begin_bar_multi → rebalance。
pub struct BacktestRunner {
    strategies: Vec<Arc<dyn Strategy>>,
    initial_cash: f64,
    force_liquidate: bool,
}

impl BacktestRunner {
    /// 构造 Runner
    ///
    /// `_engine_config` 字段保留（Phase 1 签名遗留），实际撮合配置硬编码：
    /// L1MatchingEngine + SimulatedClock + EventQueue + FeeConfig::default()。
    /// 未来需要可插拔撮合引擎时再暴露 Builder。
    pub fn new(_engine_config: BacktestEngineConfig) -> Self {
        Self {
            strategies: Vec::new(),
            initial_cash: _engine_config.initial_cash,
            force_liquidate: _engine_config.force_liquidate,
        }
    }

    /// 最简构造器（给测试/快速原型用）
    pub fn simple(initial_cash: f64) -> Self {
        Self {
            strategies: Vec::new(),
            initial_cash,
            force_liquidate: false,
        }
    }

    pub fn add_strategy(&mut self, s: Arc<dyn Strategy>) {
        self.strategies.push(s);
    }

    pub fn with_strategy(mut self, s: Arc<dyn Strategy>) -> Self {
        self.strategies.push(s);
        self
    }

    /// 全 Rust 原生回测入口
    ///
    /// # 参数
    /// - `bars`: 时间窗口内所有 K 线（必须按时间升序）
    /// - `state_by_symbol`: 每个 symbol 预计算好的 state JSON（features + window）
    ///
    /// # 返回
    /// BacktestResult：决策轨迹 + RunResult 聚合的撮合统计
    pub async fn run(
        &self,
        bars: &[Bar],
        state_by_symbol: &BTreeMap<String, serde_json::Value>,
        _initial_cash: f64,
    ) -> BacktestResult {
        assert!(!bars.is_empty(), "bars must not be empty");

        let initial_cash = self.initial_cash;

        // 1) 收集策略涉及的所有 symbol（去重）
        let unique_symbols: Vec<String> = {
            let mut set = BTreeMap::new();
            for s in &self.strategies {
                set.entry(s.symbol().to_string()).or_insert_with(|| ());
            }
            set.into_keys().collect()
        };

        // 2) 为每个 symbol 预构造 Instrument（复用，避免每 bar 重复 parse）
        let instruments: BTreeMap<String, Instrument> = unique_symbols
            .iter()
            .map(|sym| {
                let inst = Instrument::from_symbol(&Symbol::from(sym.as_str()));
                (sym.clone(), inst)
            })
            .collect();

        // 3) 构造 BacktestEngine
        let first_bar_ts = bars[0].timestamp;
        let mut engine = BacktestEngine::new(
            BacktestEngineConfig {
                clock: SimulatedClock::new(first_bar_ts),
                matching_engine: Box::new(L1MatchingEngine::new()),
                impact_model: None,
                initial_cash,
                fee_config: FeeConfig::default(),
                force_liquidate: self.force_liquidate,
            },
            EventQueue::new(),
        );
        // 启用自动 rebalance（begin_bar_multi 末尾会触发）
        engine.with_auto_rebalance(REBALANCE_THRESHOLD);

        // 4) 逐 bar 编排
        let mut decisions_timeline: Vec<BarDecisionRecord> =
            Vec::with_capacity(bars.len() * self.strategies.len().max(1));

        for (bar_idx, bar) in bars.iter().enumerate() {
            // 4a) 时钟推进到当前 bar（BacktestEngine.config 私有，走公开 setter）
            engine.set_clock(bar.timestamp);

            // 4b) 所有策略先 decide + set_target_position
            for strategy in &self.strategies {
                let state = state_by_symbol
                    .get(strategy.symbol())
                    .cloned()
                    .unwrap_or_default();

                let ctx = BarContext {
                    symbol: strategy.symbol(),
                    bar_index: bar_idx as u64,
                    // Phase 2 修复：用真实纳秒戳，之前是 RFC3339 字符串长度占位
                    timestamp_ns: bar.timestamp.nanos as u64,
                    state: &state,
                };

                let decision = strategy.decide(&ctx).await;

                // 先把 Result 转成可序列化的 timeline record（strategy.decide 返回值可能非 Clone）
                let decision_for_timeline = match &decision {
                    Ok(dec) => Ok(dec.clone()),
                    Err(e) => Err(e.to_string()),
                };

                // 无论决策结果如何都记录轨迹
                decisions_timeline.push(BarDecisionRecord {
                    bar_index: bar_idx as u64,
                    strategy: strategy.name().into(),
                    symbol: strategy.symbol().into(),
                    decision: decision_for_timeline,
                    bar_close: bar.close.as_f64(),
                });

                // 仅 Ok 时 set_target_position（引擎用它撮合）
                if let Ok(ref dec) = decision {
                    let target = action_to_target(dec.action);
                    if let Some(inst) = instruments.get(strategy.symbol()) {
                        engine.set_target_position(inst.clone(), target);
                    }
                }
            }

            // 4c) begin_bar_multi → seed_liquidity + 自动 rebalance_to_target
            // bar.close 作为 mid_price（L1 撮合引擎的 mark）
            let legs: Vec<(Instrument, f64)> = instruments
                .values()
                .map(|inst| (inst.clone(), bar.close.as_f64()))
                .collect();
            engine.begin_bar_multi(legs);
        }

        // 5) run() 消费 EventQueue + 聚合 RunResult（队列空时也会 build_result 汇总撮合统计）
        let run_result = engine.run();

        BacktestResult {
            events_processed: run_result.events_processed,
            orders_accepted: run_result.orders_accepted,
            orders_rejected: run_result.orders_rejected,
            fills: run_result.fills,
            total_pnl: run_result.total_pnl,
            max_drawdown: run_result.max_drawdown,
            final_nav: run_result.final_nav,
            decisions_timeline,
            bar_nav_curve: run_result.bar_nav_curve,
            n_bars: bars.len() as u64,
            n_strategies: self.strategies.len() as u32,
        }
    }
}

// ── 辅助：Action → target_position 映射 ── //

/// SDM/Cascade Action → BacktestEngine target_position（f64，+1.0 = 全多仓，-1.0 = 全空仓）
///
/// ponytail: Phase 2 只用 Buy/Sell/Hold 三态映射 +-1/0；
/// 未来接入 SDM position_size 答案后会做分档（0pct/25pct/50pct/100pct）。
fn action_to_target(act: axon_llm::decision::Action) -> f64 {
    use axon_llm::decision::Action::*;
    match act {
        Buy => 1.0,
        Sell => -1.0,
        Hold => 0.0,
    }
}

// ── 结果类型 ── //

/// BacktestRunner 最终输出（扩展自 Phase 1，加入 RunResult 撮合统计）
#[derive(Debug, Clone, Serialize)]
pub struct BacktestResult {
    /// 已处理事件总数（RunResult 聚合）
    pub events_processed: u64,
    /// 接受的订单数
    pub orders_accepted: u64,
    /// 拒绝的订单数
    pub orders_rejected: u64,
    /// 成交（fill）总数
    pub fills: u64,
    /// 最终 PnL（final_nav - initial_cash）
    pub total_pnl: f64,
    /// 最大回撤（USD）
    pub max_drawdown: f64,
    /// 最终净资产
    pub final_nav: f64,
    /// 每 bar 所有策略的决策轨迹
    pub decisions_timeline: Vec<BarDecisionRecord>,
    /// 每 bar 末 mark-to-market NAV 曲线（L1/L2 指标用这个）
    pub bar_nav_curve: Vec<(Timestamp, f64)>,
    /// 回测 bar 数
    pub n_bars: u64,
    /// 策略数
    pub n_strategies: u32,
}

/// 单 bar × 单策略的决策记录
#[derive(Debug, Clone, Serialize)]
pub struct BarDecisionRecord {
    pub bar_index: u64,
    pub strategy: String,
    pub symbol: String,
    #[serde(serialize_with = "ser_decision")]
    pub decision: Result<StrategyDecision, String>,
    pub bar_close: f64,
}

fn ser_decision<S>(d: &Result<StrategyDecision, String>, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match d {
        Ok(dec) => serde::Serialize::serialize(dec, s),
        Err(e) => s.serialize_str(&format!("error: {e}")),
    }
}

// ── Symbol 构造小工具 ── //

#[allow(dead_code)]
fn parse_base_symbol(market: &str) -> String {
    const QUOTES: &[&str] = &["USDT", "USDC", "USD", "USDB", "KRW", "BTC"];
    let upper = market.to_uppercase();
    for q in QUOTES {
        if upper.ends_with(q) && upper.len() > q.len() {
            return upper[..upper.len() - q.len()].to_string();
        }
    }
    upper
}
