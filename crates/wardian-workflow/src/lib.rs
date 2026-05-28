//! `wardian-workflow` is the single gate for Workflow Engine v2 blueprints.
//!
//! It parses, validates, normalizes, diffs, and round-trips the declarative
//! `.md` blueprint, and owns the Node Type Registry that every other surface
//! (builder, CLI, docs) is generated from.

pub mod blueprint;
pub mod diff;
pub mod error;
pub mod field_type;
pub mod normalize;
pub mod parse;
pub mod projections;
pub mod registry;
pub mod validate;

pub use blueprint::{Blueprint, Edge, Node};
pub use error::{Result, WorkflowError};

#[cfg(test)]
fn _crate_compiles() {}
