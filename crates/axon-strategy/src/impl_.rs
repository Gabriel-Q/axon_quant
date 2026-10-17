//! Strategy trait 内置实现：
//! - SdmStrategyImpl   — 快路径（DecisionBackend + Cascade + 可选 Harness）
//! - RuleStrategyImpl  — 纯规则兜底
//! - SwarmRouteStrategyImpl — 快慢决策路由（SDM 直出 + 低置信升级）

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use axon_llm::decision::*;
use serde::Deserialize;
use serde_json::Value;

use crate::trait_::{BarContext, Strategy, StrategyDecision, StrategyError};

// ──────────────────────────────────────────────────────────────────────
// 1. SdmStrategyImpl — SDM 快路径 + Cascade + 可选 Harness
// ──────────────────────────────────────────────────────────────────────

/// SDM 策略配置（可 JSON 序列化下发）
#[derive(Debug, Clone, Deserialize)]
pub struct SdmStrategyConfig {
    pub name: String,
    pub symbol: String,
    pub cost_threshold: f64,
    #[serde(default)]
    pub open_threshold: f64,
    #[serde(default)]
    pub hold_threshold: f64,
    /// 问题模板版本：v1（4 问题）/ v2（8 问题）
    #[serde(default = "default_version")]
    pub features_version: String,
}

fn default_version() -> String {
    "v2".into()
}

/// SDM 快路径策略
///
/// 把 DecisionBackend + SdmCascadeDecisionLayer + (可选) HarnessBridge
/// 串成一个 Strategy。决策链路：SDM → Cascade gate → (可选) Harness → Action。
pub struct SdmStrategyImpl {
    name: String,
    symbol: String,
    cascade: Arc<SdmCascadeDecisionLayer>,
    questions: BTreeMap<String, Question>,
    #[allow(dead_code)]
    features_version: String,
}

impl SdmStrategyImpl {
    pub fn new(
        config: &SdmStrategyConfig,
        backend: Arc<dyn DecisionBackend>,
        questions_raw: BTreeMap<String, Value>,
    ) -> Result<Self, String> {
        let cascade = Arc::new(SdmCascadeDecisionLayer::new(
            backend,
            Arc::new(RuleFallback::new()) as Arc<dyn FallbackDecider>,
            CascadeConfig {
                open_threshold: config.open_threshold.max(0.01),
                hold_threshold: config.hold_threshold.max(0.01),
            },
        ));
        let questions: BTreeMap<String, Question> = questions_raw
            .into_iter()
            .map(|(k, v)| serde_json::from_value::<Question>(v).map(|q| (k, q)))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("invalid question schema: {e}"))?;
        Ok(Self {
            name: config.name.clone(),
            symbol: config.symbol.clone(),
            cascade,
            questions,
            features_version: config.features_version.clone(),
        })
    }
}

#[async_trait]
impl Strategy for SdmStrategyImpl {
    fn name(&self) -> &str {
        &self.name
    }
    fn symbol(&self) -> &str {
        &self.symbol
    }

    async fn decide(&self, ctx: &BarContext<'_>) -> Result<StrategyDecision, StrategyError> {
        let req = DecisionRequest {
            state: ctx.state.clone(),
            questions: self.questions.clone(),
        };
        // ponytail: Backend 所有失败已被 CascadeDecisionLayer 内部捕获并转为 Fallback，
        // 所以这里 Result 层理论上不会 Err；但保留 match 防未来 cascade 升级。
        let cascade = self.cascade.decide(&req).await;
        Ok(StrategyDecision::from_sdm(cascade))
    }
}

// ──────────────────────────────────────────────────────────────────────
// 2. RuleStrategyImpl — 纯规则兜底
// ──────────────────────────────────────────────────────────────────────

/// 纯规则策略（双均线 + 近一根收益同向才开仓）
///
/// 用途：DecisionBackend 还没接通时的保底；或 SwarmRouteStrategyImpl 的慢路径兜底。
pub struct RuleStrategyImpl {
    name: String,
    symbol: String,
}

impl RuleStrategyImpl {
    pub fn new(name: impl Into<String>, symbol: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            symbol: symbol.into(),
        }
    }
}

#[async_trait]
impl Strategy for RuleStrategyImpl {
    fn name(&self) -> &str {
        &self.name
    }
    fn symbol(&self) -> &str {
        &self.symbol
    }

    async fn decide(&self, ctx: &BarContext<'_>) -> Result<StrategyDecision, StrategyError> {
        let feat = |name: &str| -> f64 {
            ctx.state
                .get("features")
                .and_then(|f| f.get(name))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
        };
        let sma = feat("sma5_vs_sma20");
        let ret = feat("ret_1");
        let action = if sma > 0.0 && ret > 0.0 {
            Action::Buy
        } else if sma < 0.0 && ret < 0.0 {
            Action::Sell
        } else {
            Action::Hold
        };
        Ok(StrategyDecision::from_rule(action))
    }
}

// ──────────────────────────────────────────────────────────────────────
// 3. SwarmRouteStrategyImpl — 快慢决策路由
// ──────────────────────────────────────────────────────────────────────

/// 快慢路由策略：每根 bar 先跑 SDM（快路径，70ms 级），低置信度时升级
/// 到另一个 Strategy（LLM 慢路径 / 规则兜底）。
///
/// 关键：SDM 的 sub_signals 直接传给慢路径 Strategy——LLM 不需要重新计算
/// volatility_regime / drawdown_risk 等指标，信息复用不浪费。
pub struct SwarmRouteStrategyImpl {
    name: String,
    symbol: String,
    fast: Arc<dyn Strategy>,
    slow: Arc<dyn Strategy>,
    /// 低于此置信度升级到慢路径
    upgrade_threshold: f64,
}

impl SwarmRouteStrategyImpl {
    pub fn new(
        name: impl Into<String>,
        symbol: impl Into<String>,
        fast: Arc<dyn Strategy>,
        slow: Arc<dyn Strategy>,
        upgrade_threshold: f64,
    ) -> Self {
        Self {
            name: name.into(),
            symbol: symbol.into(),
            fast,
            slow,
            upgrade_threshold,
        }
    }
}

#[async_trait]
impl Strategy for SwarmRouteStrategyImpl {
    fn name(&self) -> &str {
        &self.name
    }
    fn symbol(&self) -> &str {
        &self.symbol
    }

    async fn decide(&self, ctx: &BarContext<'_>) -> Result<StrategyDecision, StrategyError> {
        // 快路径：每根 bar 必跑
        let fast = self.fast.decide(ctx).await?;

        // 置信度达标 → 直接返回，升级零开销
        if fast.confidence >= self.upgrade_threshold {
            return Ok(fast);
        }

        // 低置信度 → 升级到慢路径
        // ponytail: 不把 fast.sub_signals 重新塞进 state——slow Strategy 可以独立重算
        // 或未来通过 BarContext 扩展字段传递 sub_signals。当前版本保持两策略解耦。
        let slow = self.slow.decide(ctx).await?;
        Ok(slow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_ctx<'a>(state: &'a Value) -> BarContext<'a> {
        BarContext {
            symbol: "BTCUSDT",
            bar_index: 42,
            timestamp_ns: 1713300000000000000,
            state,
        }
    }

    #[tokio::test]
    async fn rule_strategy_follows_sma_ret_direction() {
        let s = RuleStrategyImpl::new("rule1", "BTCUSDT");

        // 同向正 → buy
        let state = json!({"features": {"sma5_vs_sma20": 0.02, "ret_1": 0.01}});
        let d = s.decide(&sample_ctx(&state)).await.unwrap();
        assert_eq!(d.action, Action::Buy);
        assert_eq!(d.source, "rule");

        // 同向负 → sell
        let state = json!({"features": {"sma5_vs_sma20": -0.02, "ret_1": -0.01}});
        let d = s.decide(&sample_ctx(&state)).await.unwrap();
        assert_eq!(d.action, Action::Sell);

        // 矛盾 → hold
        let state = json!({"features": {"sma5_vs_sma20": 0.02, "ret_1": -0.01}});
        let d = s.decide(&sample_ctx(&state)).await.unwrap();
        assert_eq!(d.action, Action::Hold);
    }

    #[tokio::test]
    async fn swarm_route_upgrades_to_slow_when_low_confidence() {
        // fast: 永远返回 hold, confidence=0.5（低于 0.7 阈值）
        struct AlwaysLowConf;
        #[async_trait]
        impl Strategy for AlwaysLowConf {
            fn name(&self) -> &str {
                "low"
            }
            fn symbol(&self) -> &str {
                "X"
            }
            async fn decide(&self, _: &BarContext<'_>) -> Result<StrategyDecision, StrategyError> {
                Ok(StrategyDecision {
                    action: Action::Hold,
                    confidence: 0.5,
                    source: "low".into(),
                    probabilities: None,
                    sub_signals: BTreeMap::new(),
                    reason: None,
                })
            }
        }

        // slow: 永远返回 buy
        struct AlwaysBuy;
        #[async_trait]
        impl Strategy for AlwaysBuy {
            fn name(&self) -> &str {
                "buyer"
            }
            fn symbol(&self) -> &str {
                "X"
            }
            async fn decide(&self, _: &BarContext<'_>) -> Result<StrategyDecision, StrategyError> {
                Ok(StrategyDecision::from_rule(Action::Buy))
            }
        }

        let route = SwarmRouteStrategyImpl::new(
            "route1",
            "BTCUSDT",
            Arc::new(AlwaysLowConf),
            Arc::new(AlwaysBuy),
            0.7, // 置信度 < 0.7 就升级
        );
        let state = json!({"features": {}});
        let d = route.decide(&sample_ctx(&state)).await.unwrap();
        assert_eq!(
            d.action,
            Action::Buy,
            "should have upgraded to slow strategy"
        );
    }

    #[tokio::test]
    async fn swarm_route_returns_fast_when_confidence_high() {
        struct AlwaysHighConf;
        #[async_trait]
        impl Strategy for AlwaysHighConf {
            fn name(&self) -> &str {
                "high"
            }
            fn symbol(&self) -> &str {
                "X"
            }
            async fn decide(&self, _: &BarContext<'_>) -> Result<StrategyDecision, StrategyError> {
                Ok(StrategyDecision {
                    action: Action::Sell,
                    confidence: 0.95,
                    source: "high".into(),
                    probabilities: None,
                    sub_signals: BTreeMap::new(),
                    reason: None,
                })
            }
        }
        struct NeverCalled;
        #[async_trait]
        impl Strategy for NeverCalled {
            fn name(&self) -> &str {
                "never"
            }
            fn symbol(&self) -> &str {
                "X"
            }
            async fn decide(&self, _: &BarContext<'_>) -> Result<StrategyDecision, StrategyError> {
                panic!("slow should not be called");
            }
        }

        let route = SwarmRouteStrategyImpl::new(
            "route2",
            "BTCUSDT",
            Arc::new(AlwaysHighConf),
            Arc::new(NeverCalled),
            0.7,
        );
        let state = json!({"features": {}});
        let d = route.decide(&sample_ctx(&state)).await.unwrap();
        assert_eq!(d.action, Action::Sell);
    }
}
