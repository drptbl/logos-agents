use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Context;
use logos_agents::{provider, server::AgentServer};
use logos_mcp::standalone::{App, run};

fn app() -> App {
    App {
        name: "logos-agents",
        version: env!("CARGO_PKG_VERSION"),
        about: "MCP server that lets AI agents hire each other over Logos, with every logos-mcp tool. `logos-agents provide <offers.toml>` runs offers on their own instead, with no MCP client.",
        log_filter: "logos_mcp=info,logos_agents=info",
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logos_agents::check_env().map_err(anyhow::Error::msg)?;
    let mut args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "provide") {
        let file = args
            .get(2)
            .cloned()
            .context("usage: logos-agents provide <offers.toml> [options]")?;
        args.drain(1..3);
        return provide(PathBuf::from(file), args).await;
    }
    run(app(), AgentServer::new).await
}

/// Run the offers in `file` with no MCP client, until stopped.
async fn provide(file: PathBuf, args: Vec<OsString>) -> anyhow::Result<()> {
    let offers = provider::load_offers(&file).map_err(anyhow::Error::msg)?;
    let server = AgentServer::new(logos_mcp::standalone::server(&app(), args)?);
    server.start_follow_through();
    let offering = server.clone();
    let advertising = tokio::spawn(async move {
        offering
            .keep_offers(
                &offers,
                || true,
                |applied| {
                    for (id, result) in applied.as_object().into_iter().flatten() {
                        match result.get("error") {
                            Some(e) => tracing::warn!("offer {id} not advertised yet: {e}"),
                            None => tracing::info!("offer {id} advertised"),
                        }
                    }
                    true
                },
            )
            .await;
    });
    stopped().await;
    advertising.abort();
    // Stop advertising, so buyers don't find an offer nobody serves. A stop
    // that fails is tried twice more, briefly: the process is on its way out.
    for _ in 0..3 {
        let results = server.apply_offers(&[]).await;
        let failed = results
            .as_object()
            .is_some_and(|all| all.values().any(|r| r.get("error").is_some()));
        if !failed {
            break;
        }
        tracing::warn!("offers not all withdrawn: {results}");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    // Now, not once a handler still at work answers: that can take hours. A
    // restart asks it again, and the job id lets it answer as before.
    std::process::exit(0)
}

/// Ctrl-C, or SIGTERM where there is one.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
