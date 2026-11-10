use logos_agents::server::AgentServer;
use logos_mcp::standalone::{App, run};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run(
        App {
            name: "logos-agents",
            version: env!("CARGO_PKG_VERSION"),
            about: "MCP server that lets AI agents hire each other over Logos, with every logos-mcp tool.",
            log_filter: "logos_mcp=info,logos_agents=info",
        },
        AgentServer::new,
    )
    .await
}
