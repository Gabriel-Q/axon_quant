//! BacktestRunner：策略组编排入口
//!
//! 全 Rust 原生回测：逐 bar 跑所有策略的 decide()。
//! Phase 1 骨架：先跑通策略决策管线；Phase 2 再接入 BacktestEngine 撮合。

use std::collections::BTreeMap;
use std::sync::Arc;

use axon_backtest::engine::BacktestEngineConfig;
use axon_core::market::bar::Bar;
use axon_strategy::trait_::{BarContext, Strategy, StrategyDecision};
use serde::Serialize;

/// 回测运行器
pub struct BacktestRunner {
    strategies: Vec<Arc<dyn Strategy>>,
}

impl BacktestRunner {
    pub fn new(_engine_config: BacktestEngineConfig) -> Self {
        Self {
            strategies: Vec::new(),
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
    /// - bars: Vec<Bar>，时间窗口内的所有 K 线（必须按时间升序）
    /// - state_by_symbol: 每个 symbol 预计算好的 state JSON（features + window）
    /// - initial_cash: 初始资金（USDT）
    pub async fn run(
        &self,
        bars: &[Bar],
        state_by_symbol: &BTreeMap<String, serde_json::Value>,
        initial_cash: f64,
    ) -> BacktestResult {
        assert!(!bars.is_empty(), "bars must not be empty");

        let _ = initial_cash;

        let mut decisions_timeline: Vec<BarDecisionRecord> = Vec::with_capacity(bars.len());

        for (bar_idx, bar) in bars.iter().enumerate() {
            for strategy in &self.strategies {
                let state = state_by_symbol
                    .get(strategy.symbol())
                    .cloned()
                    .unwrap_or_default();

                let ctx = BarContext {
                    symbol: strategy.symbol(),
                    bar_index: bar_idx as u64,
                    timestamp_ns: bar.timestamp.to_rfc3339().len() as u64, // ponytail: timestamp_ns 预留
                    state: &state,
                };

                let decision = strategy.decide(&ctx).await;

                decisions_timeline.push(BarDecisionRecord {
                    bar_index: bar_idx as u64,
                    strategy: strategy.name().into(),
                    symbol: strategy.symbol().into(),
                    decision: decision.map_err(|e| e.to_string()),
                    bar_close: bar.close.as_f64(),
                });
            }
        }

        BacktestResult {
            events_processed: 0,
            orders_accepted: 0,
            orders_rejected: 0,
            decisions_timeline,
            n_bars: bars.len() as u64,
            n_strategies: self.strategies.len() as u32,
        }
    }
}

/// Action → BacktestEngine target_position 分档
///
/// ponytail: Phase 1 骨架先用简单映射；Phase 2 接入 BacktestEngine 时会由
/// RiskEngine 根据 SDM position_size 答案做分档。通配符处理未来 Action 变体扩展。
#[allow(dead_code)]
fn action_to_target(act: axon_llm::decision::Action) -> f64 {
    use axon_llm::decision::Action::*;
    match act {
        Buy => 1.0,
        Sell => -1.0,
        Hold => 0.0,
    }
}

// ── 结果类型 ── //

#[derive(Debug, Clone, Serialize)]
pub struct BacktestResult {
    pub events_processed: u64,
    pub orders_accepted: u64,
    pub orders_rejected: u64,
    pub decisions_timeline: Vec<BarDecisionRecord>,
    pub n_bars: u64,
    pub n_strategies: u32,
}

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
