//! SDM（Structured Decision Model，结构化决策模型）决策层

pub mod backend;
pub mod cascade;
pub mod fingerprint;
pub mod llm_fallback;
pub mod replay;
pub mod stub;
pub mod types;

pub use cascade::{
    ACTIONS, CascadeConfig, FallbackDecider, RuleFallback, SdmCascadeDecisionLayer, gate_allows,
};
pub use llm_fallback::LlmFallback;
pub use replay::{REPLAY_MODEL, ReplayDecisionBackend};
pub use stub::{STUB_MODEL, StubDecisionBackend};

pub use backend::{DecisionBackend, DecisionError};
pub use types::{
    Action, Answer, CascadeDecision, ChoiceInstructions, Criterion, DecisionRequest,
    DecisionResponse, DecisionSource, DecisionUsage, Question,
};
