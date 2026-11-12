//! logos-agents: AI agents hiring each other over Logos.
//!
//! [`server::AgentServer`] serves every logos-mcp tool plus the `service_*`
//! tools, which follow the protocol in [`protocol`] and pay through the
//! logos-escrow program in [`escrow`].

pub mod escrow;
pub mod protocol;
pub mod provider;
pub mod server;

/// Check the settings read from the env before serving, so a bad one stops
/// the server at once rather than later.
pub fn check_env() -> Result<(), String> {
    escrow::check_env()?;
    server::follow_through_every().map(|_| ())
}
