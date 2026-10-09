//! SDM 决策后端抽象与统一错误类型

/// SDM 决策调用的统一错误类型
#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    /// 网络错误（瞬时，可重试）
    #[error("network error: {0}")]
    Network(String),

    /// 认证失败（不可重试）
    #[error("auth error: {0}")]
    Auth(String),

    /// 限流（携带服务端 Retry-After 秒数，可空）
    #[error("rate limited (retry_after={retry_after:?}s)")]
    RateLimited {
        /// 服务端建议等待秒数，未知为 None
        retry_after: Option<u64>,
    },

    /// 响应解析失败（不可重试）
    #[error("parse error: {0}")]
    Parse(String),

    /// 后端通用错误（含 replay miss；5xx 可重试）
    #[error("backend error: {0}")]
    Backend(String),
}

impl DecisionError {
    /// 是否值得重试，与 `LLMError::is_retryable` 口径一致
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::RateLimited { .. } | Self::Backend(_)
        )
    }

    /// 错误判别名（用于 cascade 写 fallback_reason，不对外暴露内部结构）
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Network(_) => "Network",
            Self::Auth(_) => "Auth",
            Self::RateLimited { .. } => "RateLimited",
            Self::Parse(_) => "Parse",
            Self::Backend(_) => "Backend",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_matches_llm_error_policy() {
        assert!(DecisionError::Network("x".into()).is_retryable());
        assert!(DecisionError::RateLimited { retry_after: None }.is_retryable());
        assert!(DecisionError::Backend("5xx".into()).is_retryable());
        assert!(!DecisionError::Auth("bad key".into()).is_retryable());
        assert!(!DecisionError::Parse("bad json".into()).is_retryable());
    }

    #[test]
    fn kind_names_are_stable() {
        assert_eq!(DecisionError::Network("x".into()).kind(), "Network");
        assert_eq!(DecisionError::Parse("x".into()).kind(), "Parse");
        assert_eq!(
            DecisionError::RateLimited {
                retry_after: Some(2)
            }
            .kind(),
            "RateLimited"
        );
    }
}
