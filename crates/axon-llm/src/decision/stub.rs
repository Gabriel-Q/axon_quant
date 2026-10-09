//! 确定性模拟决策后端（model = "sdm-stub"）
//!
//! ponytail: 用极简线性强度 x（双均线偏离 + 短周期收益，按波动率归一）驱动 softmax；
//! 天花板是不模拟真实 SDM 的判断偏差，用途仅限打通链路与 CI。升级路径是录制真实响应。

use async_trait::async_trait;
use serde_json::json;

use super::backend::{DecisionBackend, DecisionError};
use super::types::{DecisionRequest, DecisionResponse};

/// 模型标识
pub const STUB_MODEL: &str = "sdm-stub";

/// 确定性模拟后端
pub struct StubDecisionBackend;

impl StubDecisionBackend {
    /// 构造
    pub fn new() -> Self {
        Self
    }
}

impl Default for StubDecisionBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn softmax(values: [f64; 3]) -> [f64; 3] {
    let top = values[0].max(values[1]).max(values[2]);
    let exps = values.map(|v| (v - top).exp());
    let total: f64 = exps.iter().sum();
    exps.map(|v| v / total)
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    lo.max(hi.min(x))
}

fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn feature<'a>(state: &'a serde_json::Value, name: &str) -> Result<f64, DecisionError> {
    state
        .get("features")
        .and_then(|f| f.get(name))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| DecisionError::Backend(format!("state.features.{name} missing")))
}

#[async_trait]
impl DecisionBackend for StubDecisionBackend {
    async fn decide(&self, req: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        let f = &req.state;
        let vol = feature(f, "vol_20")?.max(1e-9);
        let sma = feature(f, "sma5_vs_sma20")?;
        let ret = feature(f, "ret_1")?;
        let volz = feature(f, "volume_z")?;

        let x = clamp(sma / vol, -3.0, 3.0) + 0.5 * clamp(ret / vol, -2.0, 2.0);
        // 顺序固定：buy, sell, hold
        let probs = softmax([x, -x, -x.abs() * 1.5 + 0.5]);
        let top_idx = if probs[0] >= probs[1] && probs[0] >= probs[2] {
            0
        } else if probs[1] >= probs[2] {
            1
        } else {
            2
        };
        let names = ["buy", "sell", "hold"];
        let choice_name = names[top_idx];
        let top_p = round6(probs[top_idx]);
        let choice_probs = json!({
            "buy": round6(probs[0]),
            "sell": round6(probs[1]),
            "hold": round6(probs[2]),
        });

        // 清晰度等级跟随胜选项概率
        let level = if top_p >= 0.7 {
            2
        } else if top_p >= 0.5 {
            1
        } else {
            0
        };
        let mut score_probs: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for i in 0..3 {
            score_probs.insert(i.to_string(), json!(0.0));
        }
        score_probs.insert(level.to_string(), json!(0.8));
        let lower = (level - 1).max(0);
        let upper = (level + 1).min(2);
        for idx in [lower, upper] {
            let cur = score_probs[&idx.to_string()].as_f64().unwrap_or(0.0);
            score_probs.insert(idx.to_string(), json!(cur + 0.1));
        }

        let raw = json!({
            "model": STUB_MODEL,
            "answers": {
                "next_direction": {
                    "type": "choice",
                    "choice": choice_name,
                    "confidence": top_p,
                    "probabilities": choice_probs
                },
                "trend_confirms": {"type": "noul", "noul": round6(sigmoid(x.abs()))},
                "risk_regime_bad": {"type": "noul", "noul": round6(sigmoid(volz.abs() - 1.5))},
                "signal_clarity": {
                    "type": "score",
                    "score": level as f64,
                    "confidence": 0.8,
                    "probabilities": serde_json::Value::Object(score_probs)
                }
            },
            "usage": {"input_tokens": 0, "cost_usd": 0.0}
        });
        DecisionResponse::from_value(raw)
    }

    fn model_name(&self) -> &str {
        STUB_MODEL
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::Answer;
    use super::*;
    use std::collections::BTreeMap;

    fn request_with(sma: f64, ret: f64) -> DecisionRequest {
        DecisionRequest {
            state: json!({
                "features": {
                    "ret_1": ret, "ret_5": ret, "vol_20": 0.01,
                    "sma5_vs_sma20": sma, "volume_z": 0.2
                }
            }),
            questions: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn strong_up_market_picks_buy_and_is_deterministic() {
        let b = StubDecisionBackend::new();
        let r1 = b.decide(&request_with(0.02, 0.01)).await.unwrap();
        let r2 = b.decide(&request_with(0.02, 0.01)).await.unwrap();
        assert_eq!(r1, r2);
        assert_eq!(b.model_name(), STUB_MODEL);
        match r1.answers.get("next_direction").unwrap() {
            Answer::Choice { choice, .. } => assert_eq!(choice, "buy"),
            other => panic!("{other:?}"),
        }
        assert_eq!(r1.usage.cost_usd, 0.0);
    }

    #[tokio::test]
    async fn missing_feature_is_backend_error() {
        let req = DecisionRequest {
            state: json!({"features": {}}),
            questions: BTreeMap::new(),
        };
        let err = StubDecisionBackend::new()
            .decide(&req)
            .await
            .expect_err("fail");
        assert!(matches!(err, DecisionError::Backend(_)));
    }
}
