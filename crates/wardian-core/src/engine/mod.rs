//! `wardian-engine` executes a validated `wardian-automation` blueprint as a
//! durable, resumable run. The pure core (`state` + `core`) holds the logic;
//! the async `driver` performs IO and calls the dependency-inverted executor.

pub mod core;
pub mod driver;
pub mod error;
pub mod event;
pub mod executor;
pub mod graph;
pub mod interpolate;
pub mod message_artifact;
pub mod state;
pub mod store;

#[cfg(test)]
mod message_send_tests;

pub use driver::Engine;
pub use error::{EngineError, Result, StepError};
pub use event::{Event, EventKind};
pub use executor::{
    AgentTaskRequest, ChosenPort, DecisionRequest, MemoryCommitRequest, MessageSendRequest,
    MockExecutor, NotifyRequest, ScriptRequest, ShellRequest, StepExecutor, StepOutput,
};
pub use state::{NodeStatus, RunState, RunStatus};
