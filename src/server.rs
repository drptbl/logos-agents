//! The agent server: every logos-mcp tool, plus the `service_*` tools in
//! [`jobs`]. It wraps a [`LogosServer`] and passes on every call it has no
//! tool for, so the node tools behave exactly as they do in logos-mcp.

use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

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
        ChatSendParams, ChatStartParams, LogosServer, MAX_CHAT_BYTES, SendError, ServiceParams,
        TransferParams, sanitise_peer_text, service_entry,
    },
};

mod jobs;

const LEZ: &str = "lez_core";
const P2P: &str = "libp2p_module";
const CHAT: &str = "chat_module";

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
    tool_router: ToolRouter<Self>,
}

impl AgentServer {
    pub fn new(base: LogosServer) -> Self {
        Self {
            base,
            services: Arc::default(),
            quoting: Arc::default(),
            advertising: Arc::default(),
            tool_router: Self::tool_router(),
        }
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
