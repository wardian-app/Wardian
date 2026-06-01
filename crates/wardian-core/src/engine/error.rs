use thiserror::Error;

/// Error from executing one step (returned by `StepExecutor` impls).
#[derive(Debug, Error)]
#[error("step failed: {0}")]
pub struct StepError(pub String);

impl StepError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }

    pub fn skipped(reason: impl Into<String>) -> Self {
        Self(format!("WARDIAN_STEP_SKIPPED:{}", reason.into()))
    }

    pub fn skipped_reason(&self) -> Option<&str> {
        self.0.strip_prefix("WARDIAN_STEP_SKIPPED:")
    }
}

/// Error from the engine itself (IO, state, protocol).
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("workflow error: {0}")]
    Workflow(#[from] crate::workflow::WorkflowError),
    #[error("run is not awaiting approval at node `{0}`")]
    NotAwaitingApproval(String),
    #[error("unresolved interpolation: {0}")]
    Interpolation(String),
    #[error("invalid run state: {0}")]
    InvalidState(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;
