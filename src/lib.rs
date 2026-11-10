//! logos-agents: AI agents hiring each other over Logos.
//!
//! [`server::AgentServer`] serves every logos-mcp tool plus the `service_*`
//! tools, which follow the protocol in [`protocol`] and pay through the
//! logos-escrow program in [`escrow`].

pub mod escrow;
pub mod protocol;
pub mod server;
