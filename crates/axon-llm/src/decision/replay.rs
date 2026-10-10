//! 指纹精确回放后端：fingerprint → 录制响应；miss 即报错（禁止静默兜底）

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;

use super::backend::{DecisionBackend, DecisionError};
use super::fingerprint::request_fingerprint;
use super::types::{DecisionRequest, DecisionResponse};

/// 回放后端模型标识
pub const REPLAY_MODEL: &str = "replay";

/// 指纹 → 录制响应 的精确回放
pub struct ReplayDecisionBackend {
    recordings: BTreeMap<String, DecisionResponse>,
}

impl ReplayDecisionBackend {
    /// 从内存映射构造
    pub fn new(recordings: BTreeMap<String, DecisionResponse>) -> Self {
        Self { recordings }
    }

    /// 读取 JSONL，每行形如 {"fingerprint": "...", "response": {...}}
    pub fn from_jsonl(path: &Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut recordings = BTreeMap::new();
        for (lineno, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(line).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("line {}: {e}", lineno + 1),
                )
            })?;
            let fp = v
                .get("fingerprint")
                .and_then(|x| x.as_str())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("line {}: missing fingerprint", lineno + 1),
                    )
                })?;
            let resp_v = v.get("response").cloned().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("line {}: missing response", lineno + 1),
                )
            })?;
            let resp = DecisionResponse::from_value(resp_v)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            recordings.insert(fp.to_string(), resp);
        }
        Ok(Self { recordings })
    }

    /// 录制条数
    pub fn len(&self) -> usize {
        self.recordings.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.recordings.is_empty()
    }
}

#[async_trait]
impl DecisionBackend for ReplayDecisionBackend {
    async fn decide(&self, req: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        let fp = request_fingerprint(req);
        self.recordings
            .get(&fp)
            .cloned()
            .ok_or_else(|| DecisionError::Backend(format!("replay miss for fingerprint={fp}")))
    }

    fn model_name(&self) -> &str {
        REPLAY_MODEL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::io::Write;

    fn req(v: i64) -> DecisionRequest {
        DecisionRequest {
            state: json!({"n": v}),
            questions: BTreeMap::new(),
        }
    }

    fn resp_body(model: &str) -> serde_json::Value {
        json!({
            "model": model,
            "answers": {"next_direction": {"type":"choice","choice":"hold","confidence":0.9,"probabilities":{"buy":0.05,"sell":0.05,"hold":0.9}}},
            "usage": {"input_tokens": 1, "cost_usd": 0.0}
        })
    }

    #[tokio::test]
    async fn hit_returns_recorded_and_miss_errors() {
        let r1 = req(1);
        let fp = request_fingerprint(&r1);
        let mut map = BTreeMap::new();
        map.insert(fp, DecisionResponse::from_value(resp_body("m")).unwrap());
        let b = ReplayDecisionBackend::new(map);

        let got = b.decide(&r1).await.unwrap();
        assert_eq!(got.model, "m");

        let err = b.decide(&req(2)).await.expect_err("miss");
        assert!(matches!(err, DecisionError::Backend(_)));
        assert!(err.to_string().contains("replay miss"));
    }

    #[tokio::test]
    async fn from_jsonl_roundtrip() {
        let r1 = req(7);
        let fp = request_fingerprint(&r1);
        let line = json!({"fingerprint": fp, "response": resp_body("rec")}).to_string();
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "{line}").unwrap();
        let b = ReplayDecisionBackend::from_jsonl(tmp.path()).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b.decide(&r1).await.unwrap().model, "rec");
    }
}
