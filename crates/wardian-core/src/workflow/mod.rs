//! `wardian-workflow` is the single gate for Workflow Engine v2 blueprints.
//!
//! It parses, validates, normalizes, diffs, and round-trips the declarative
//! `.md` blueprint, and owns the Node Type Registry that every other surface
//! (builder, CLI, docs) is generated from.

pub mod assignment;
pub mod blueprint;
pub mod diff;
pub mod error;
pub mod field_type;
pub mod normalize;
pub mod parse;
pub mod projections;
pub mod registry;
pub mod validate;

pub use blueprint::{Blueprint, Edge, Node, Position};
pub use diff::{diff, BlueprintDiff};
pub use error::{Result, WorkflowError};
pub use field_type::{FieldDef, FieldType};
pub use normalize::normalize;
pub use parse::{parse_file, parse_str, to_string};
pub use projections::reference_doc::reference_doc;
pub use projections::ts_schema::{ts_schema_json, ts_schema_value};
pub use registry::{find_node_type, node_types, NodeKind, NodeTypeDef, PortDef};
pub use validate::{validate, Diagnostic, Severity, ValidationReport};

#[cfg(test)]
fn _crate_compiles() {}
