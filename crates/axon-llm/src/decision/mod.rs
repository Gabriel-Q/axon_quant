//! SDM（Structured Decision Model，结构化决策模型）决策层

pub mod backend;
pub mod fingerprint;
pub mod stub;
pub mod types;

pub use stub::{STUB_MODEL, StubDecisionBackend};

pub use backend::{DecisionBackend, DecisionError};
pub use types::{
    Action, Answer, CascadeDecision, ChoiceInstructions, Criterion, DecisionRequest,
    DecisionResponse, DecisionSource, DecisionUsage, Question,
};
