//! The agent server: every logos-mcp tool, plus the `service_*` tools in
//! [`jobs`]. It wraps a [`LogosServer`] and passes on every call it has no
//! tool for, so the node tools behave exactly as they do in logos-mcp.

use std::{collections::BTreeSet, sync::Arc};

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerConfig},
    schemars::{self, JsonSchema},
    tool, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use base64::Engine;
use logos_mcp::{
    control::{APPROVAL_EXPIRES, ProgramCall},
    guard,
    logosctl::{Arg, LogosError},
    server::{
        ChatSendParams, ChatStartParams, JOB_ACCOUNTS, LogosServer, MAX_CHAT_BYTES, SendError,
        ServiceParams, TransferParams, sanitise_peer_text, service_entry,
    },
};

mod jobs;

const LEZ: &str = "lez_core";
const P2P: &str = "libp2p_module";
const CHAT: &str = "chat_module";

/// How often the follow-through looks at every job, unless
/// [`FOLLOW_THROUGH_ENV`] says otherwise.
const FOLLOW_THROUGH_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Sets the follow-through's interval in seconds, 1 to 3600: a few seconds
/// suits a local devnet, whose blocks land in one.
pub const FOLLOW_THROUGH_ENV: &str = "LOGOS_AGENTS_FOLLOW_THROUGH_SECS";

/// The follow-through's interval, from [`FOLLOW_THROUGH_ENV`] or the default.
pub fn follow_through_every() -> Result<std::time::Duration, String> {
    let secs = match std::env::var(FOLLOW_THROUGH_ENV) {
        Err(std::env::VarError::NotPresent) => return Ok(FOLLOW_THROUGH_EVERY),
        Ok(secs) => secs,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{FOLLOW_THROUGH_ENV} is not UTF-8"));
        }
    };
    parse_follow_through(&secs)
}

fn parse_follow_through(secs: &str) -> Result<std::time::Duration, String> {
    secs.trim()
        .parse::<u64>()
        .ok()
        .filter(|s| (1..=3600).contains(s))
        .map(std::time::Duration::from_secs)
        .ok_or_else(|| {
            format!("{FOLLOW_THROUGH_ENV} must be whole seconds from 1 to 3600, not {secs:?}")
        })
}

fn ok(value: Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::json(value)?]))
}

/// Tool-level failure: reported to the agent as an error result it can act on.
fn fail(message: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(
        message.into(),
    )]))
}

/// A node failure, reported to the agent as an error result.
fn logos_fail(e: LogosError) -> Result<CallToolResult, McpError> {
    fail(e.to_string())
}

/// A tool's JSON reply, or its error text.
fn tool_json(result: Result<CallToolResult, McpError>) -> Result<Value, String> {
    let result = result.map_err(|e| e.message.to_string())?;
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    if result.is_error == Some(true) {
        return Err(text);
    }
    serde_json::from_str(&text).map_err(|e| format!("unreadable tool reply: {e}"))
}

/// logos-mcp's server with the service tools on top.
#[derive(Clone)]
pub struct AgentServer {
    base: LogosServer,
    /// The service tools' offers, conversations and payments in flight.
    services: Arc<tokio::sync::Mutex<jobs::Services>>,
    /// Held while quoting, so a job never gets two accounts.
    quoting: Arc<tokio::sync::Mutex<()>>,
    /// Held while offers are advertised or stopped, so they don't cross.
    advertising: Arc<tokio::sync::Mutex<()>>,
    /// Set once the follow-through runs: one loop for a server and its clones.
    following: Arc<std::sync::OnceLock<()>>,
    tool_router: ToolRouter<Self>,
}

impl AgentServer {
    pub fn new(base: LogosServer) -> Self {
        Self {
            base,
            services: Arc::default(),
            quoting: Arc::default(),
            advertising: Arc::default(),
            following: Arc::default(),
            tool_router: Self::tool_router(),
        }
    }

    /// Take the steps that only wait on the chain, for every job, every
    /// [`follow_through_every`] from now on: see [`jobs`]. Runs once however often it is
    /// called, until the tokio runtime it was called in stops.
    pub fn start_follow_through(&self) {
        if self.following.set(()).is_err() {
            return;
        }
        let server = self.clone();
        tokio::spawn(async move {
            // Checked at startup with the rest of the env: see `check_env`.
            let mut every =
                tokio::time::interval(follow_through_every().unwrap_or(FOLLOW_THROUGH_EVERY));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                server.follow_through().await;
            }
        });
    }

    /// Follow the user's tool switches, as logos-mcp's server does.
    pub fn with_tool_groups(
        self,
        groups: tokio::sync::watch::Receiver<logos_mcp::control::ToolGroups>,
    ) -> Self {
        Self {
            base: self.base.with_tool_groups(groups),
            ..self
        }
    }
}

#[cfg(test)]
mod tests;

impl ServerHandler for AgentServer {
    async fn on_initialized(&self, context: rmcp::service::NotificationContext<rmcp::RoleServer>) {
        self.base.on_initialized(context).await;
        self.start_follow_through();
    }

    async fn list_tools(
        &self,
        request: Option<rmcp::model::PaginatedRequestParams>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, McpError> {
        let mut listed = self.base.list_tools(request, context).await?;
        // A tool this server has replaces logos-mcp's of the same name.
        listed
            .tools
            .retain(|t| !self.tool_router.has_route(&t.name));
        listed.tools.extend(
            self.tool_router
                .list_all()
                .into_iter()
                .filter(|t| self.base.allows(&t.name)),
        );
        Ok(listed)
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        if self.tool_router.has_route(name) {
            self.base
                .allows(name)
                .then(|| self.tool_router.get(name).cloned())
                .flatten()
        } else {
            self.base.get_tool(name)
        }
    }

    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, McpError> {
        if self.tool_router.has_route(&request.name) {
            self.base
                .guarded_call(&self.tool_router, self, request, context)
                .await
        } else {
            self.base.call_tool(request, context).await
        }
    }

    fn get_info(&self) -> ServerConfig {
        let info = self.base.get_info();
        let instructions = format!(
            "{} Agents hire each other with the service_* tools: service_jobs always says what to call next.",
            info.instructions.clone().unwrap_or_default()
        );
        info.with_instructions(instructions)
    }
}
