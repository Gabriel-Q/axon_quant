//! SDM 真实 HTTP 后端（`decision-http` feature）
//!
//! - 端点：`{base_url}/v1/decide`，Bearer 鉴权
//! - 请求体：`{"model": ..., "state": ..., "questions": ...}`
//! - 复用 `backends::retry::with_backoff`（闭包内用 LLMError，边界映射 DecisionError）

use std::time::Duration;

use async_trait::async_trait;

use crate::backend::LLMError;
use crate::backends::retry::{BackoffConfig, with_backoff};

use super::backend::{DecisionBackend, DecisionError};
use super::types::{DecisionRequest, DecisionResponse};

/// SDM HTTP 后端配置（显式传参，不读环境变量，对齐 LLMConfig 风格）
#[derive(Debug, Clone)]
pub struct SdmHttpConfig {
    /// 基础 URL（不带尾斜杠/路径）
    pub base_url: String,
    /// API Key
    pub api_key: String,
    /// 模型标识
    pub model: String,
    /// 超时秒数
    pub timeout_secs: u64,
    /// 最大重试次数（不含首次）
    pub max_retries: u32,
}

/// SDM 真实 HTTP 后端
pub struct SdmHttpBackend {
    client: reqwest::Client,
    cfg: SdmHttpConfig,
}

impl SdmHttpBackend {
    /// 构造（构建 client 失败返回 Network 错误）
    pub fn new(cfg: SdmHttpConfig) -> Result<Self, DecisionError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_secs.max(1)))
            .build()
            .map_err(|e| DecisionError::Network(e.to_string()))?;
        Ok(Self { client, cfg })
    }

    fn url(&self) -> String {
        format!("{}/v1/decide", self.cfg.base_url.trim_end_matches('/'))
    }
}

fn status_to_llm_error(status: u16, body: String) -> LLMError {
    match status {
        401 | 403 => LLMError::Auth(body),
        429 => LLMError::RateLimited { retry_after: None },
        s if (500..600).contains(&s) => LLMError::Backend(body),
        s => LLMError::Backend(format!("unexpected status {s}: {body}")),
    }
}

#[async_trait]
impl DecisionBackend for SdmHttpBackend {
    async fn decide(&self, req: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        let url = self.url();
        let body = serde_json::json!({
            "model": self.cfg.model,
            "state": req.state,
            "questions": req.questions,
        });
        let backoff = BackoffConfig {
            max_retries: self.cfg.max_retries,
            ..BackoffConfig::default()
        };
        let text: String = with_backoff(backoff, || {
            let client = self.client.clone();
            let url = url.clone();
            let key = self.cfg.api_key.clone();
            let body = body.clone();
            async move {
                let resp = client
                    .post(&url)
                    .bearer_auth(key)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| LLMError::Network(e.to_string()))?;
                let status = resp.status().as_u16();
                let text = resp
                    .text()
                    .await
                    .map_err(|e| LLMError::Network(e.to_string()))?;
                if status == 200 {
                    Ok(text)
                } else {
                    Err(status_to_llm_error(status, text))
                }
            }
        })
        .await
        .map_err(super::llm_fallback::map_llm_error)?;

        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| DecisionError::Parse(e.to_string()))?;
        DecisionResponse::from_value(v)
    }

    fn model_name(&self) -> &str {
        &self.cfg.model
    }
}

#[cfg(test)]
mod tests {
    // wiremock 端到端：需 decision-http（本模块存在）+ e2e（提供 wiremock）
    #[cfg(feature = "e2e")]
    #[tokio::test]
    async fn posts_and_parses_decision_response() {
        use super::*;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let body = serde_json::json!({
            "model": "sdm-x",
            "answers": {"next_direction": {"type":"choice","choice":"buy","confidence":0.9,"probabilities":{"buy":0.9,"sell":0.05,"hold":0.05}}},
            "usage": {"input_tokens": 10, "cost_usd": 0.002}
        });
        Mock::given(method("POST"))
            .and(path("/v1/decide"))
            .and(header("authorization", "Bearer secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let cfg = SdmHttpConfig {
            base_url: server.uri(),
            api_key: "secret".into(),
            model: "sdm-x".into(),
            timeout_secs: 5,
            max_retries: 0,
        };
        let backend = SdmHttpBackend::new(cfg).unwrap();
        let req = DecisionRequest {
            state: serde_json::json!({"features": {}}),
            questions: std::collections::BTreeMap::new(),
        };
        let resp = backend.decide(&req).await.unwrap();
        assert_eq!(resp.model, "sdm-x");
        assert_eq!(backend.model_name(), "sdm-x");
    }

    #[cfg(feature = "e2e")]
    #[tokio::test]
    async fn maps_5xx_to_backend_error() {
        use super::*;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/decide"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let cfg = SdmHttpConfig {
            base_url: server.uri(),
            api_key: "k".into(),
            model: "m".into(),
            timeout_secs: 5,
            max_retries: 0,
        };
        let err = SdmHttpBackend::new(cfg)
            .unwrap()
            .decide(&DecisionRequest {
                state: serde_json::json!({}),
                questions: std::collections::BTreeMap::new(),
            })
            .await
            .expect_err("5xx");
        assert!(matches!(err, DecisionError::Backend(_)), "got {err:?}");
    }
}
