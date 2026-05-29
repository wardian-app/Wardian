//! `wardian-engine` executes a validated `wardian-workflow` blueprint as a
//! durable, resumable run. The pure core (`state` + `core`) holds the logic;
//! the async `driver` performs IO and calls the dependency-inverted executor.

pub mod core;
pub mod driver;
pub mod error;
pub mod event;
pub mod executor;
pub mod graph;
pub mod interpolate;
pub mod state;
pub mod store;

pub use driver::Engine;
pub use error::{EngineError, Result, StepError};
pub use event::{Event, EventKind};
pub use executor::{
    AgentTaskRequest, ChosenPort, DecisionRequest, MockExecutor, NotifyRequest, ScriptRequest,
    ShellRequest, StateRequest, StepExecutor, StepOutput,
};
pub use state::{NodeStatus, RunState, RunStatus};
