//! agentdisk core: measuring, agent adapters, safety rules and cleanup.
//!
//! Design rules:
//! - Scanning never writes anything.
//! - Cleanup moves to quarantine first; deletion is a separate, later step.
//! - Only a human in a terminal can apply a plan. The MCP server has no
//!   tool for it.

pub mod adapters;
pub mod advise;
pub mod check;
pub mod cleanup;
pub mod compact;
pub mod config;
pub mod git;
pub mod json;
pub mod mcp;
pub mod paths;
pub mod projects;
pub mod scan;
pub mod size;
pub mod sys;
pub mod util;

#[cfg(test)]
pub(crate) mod testutil;

/// Version of agentdisk, taken from Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
