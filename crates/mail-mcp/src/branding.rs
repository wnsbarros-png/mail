//! The single place that names the product for the AI integration.
//!
//! TRADEMARKS.md forbids shipping a fork under the upstream name and logo.
//! When the fork's final name is chosen, change these constants (and the
//! package metadata) instead of hunting for strings across the crate.

/// Human-readable application name reported to MCP clients.
pub const APP_NAME: &str = "Mail (custom build)";
/// Machine-readable MCP server name.
pub const SERVER_NAME: &str = "mail-mcp";
/// File, inside the application's data directory, that holds AI settings.
pub const CONFIG_FILE: &str = "ai-mcp.json";
/// Lock file that keeps a second MCP server from running on the same profile.
pub const LOCK_FILE: &str = "ai-mcp.lock";
