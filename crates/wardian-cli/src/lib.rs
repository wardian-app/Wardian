//! Shared Wardian MCP server used by both CLI entry points.

pub mod control_client;
pub mod mcp;
#[cfg(test)]
mod test_support;
