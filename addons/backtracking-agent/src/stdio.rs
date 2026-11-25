//! Stdio mode (`--stdio`): what an MCP client starts. It answers `initialize` and
//! `tools/list` itself, so a client always connects and sees the tools, and passes every tool
//! call to the running add-on's HTTP endpoint, found through the connection file. When
//! Backtracking or the add-on is not running, a call says so at once.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ListToolsResult, PaginatedRequestParams, ServerConfig,
};
use rmcp::service::{Peer, RequestContext, RunningService};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ErrorData as McpError, RoleClient, RoleServer, ServerHandler, ServiceExt};
use tokio::sync::Mutex;

use crate::endpoint::Endpoint;
use crate::tools::{self, AgentServer};

/// How long connecting to the add-on may take.
const CONNECT_WITHIN: Duration = Duration::from_secs(10);
/// Tools that change nothing and can safely be sent again after a lost connection.
const READ_ONLY: [&str; 2] = ["get_state", "list_sources"];

pub fn run(endpoint_path: PathBuf) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let result = runtime.block_on(async move {
        let forwarder = Forwarder {
            endpoint_path,
            connection: Arc::new(Mutex::new(None)),
        };
        let connections = forwarder.connection.clone();
        let service = forwarder
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|error| error.to_string())?;
        // Ends when the client closes stdin.
        let _ = service.waiting().await;
        if let Some(connection) = connections.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), connection.service.cancel()).await;
        }
        Ok(())
    });
    // A blocking read of stdin cannot be cancelled; nothing is left to do, so do not wait.
    runtime.shutdown_timeout(Duration::from_millis(200));
    result
}

struct Connection {
    /// The connection file's contents this connection was made from.
    endpoint: Endpoint,
    service: RunningService<RoleClient, ()>,
}

#[derive(Clone)]
struct Forwarder {
    endpoint_path: PathBuf,
    connection: Arc<Mutex<Option<Connection>>>,
}

/// Use a parsed numeric socket address as the only source for the outgoing URL.
fn local_mcp_url(url: &str) -> Option<String> {
    let authority = url.strip_prefix("http://")?.strip_suffix("/mcp")?;
    let addr: std::net::SocketAddr = authority.parse().ok()?;
    (addr.ip().is_loopback() && addr.port() != 0).then(|| format!("http://{addr}/mcp"))
}

impl Forwarder {
    fn not_running(&self, why: &str) -> String {
        format!(
            "Backtracking is not running, or its Backtracking Agent add-on is not switched on \
             ({why}). Start Backtracking and check that Backtracking Agent is enabled in Options \
             > Add-ons, then try again. (Connection file: {})",
            self.endpoint_path.display()
        )
    }

    /// A peer for the add-on now running, connecting again when the connection file changed
    /// (Backtracking restarted, the port moved, or the token was renewed).
    async fn peer(&self) -> Result<Peer<RoleClient>, String> {
        let endpoint = match Endpoint::read(&self.endpoint_path) {
            Ok(endpoint) => endpoint,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.drop_connection().await;
                return Err(self.not_running("no connection file"));
            }
            Err(error) => return Err(self.not_running(&format!("the connection file: {error}"))),
        };
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref() {
            if connection.endpoint == endpoint && !connection.service.is_transport_closed() {
                return Ok(connection.service.peer().clone());
            }
        }
        if let Some(old) = slot.take() {
            let _ = old.service.cancel().await;
        }
        if !endpoint.is_current(&self.endpoint_path) {
            return Err(self.not_running("the connection file is from a run that has ended"));
        }
        let url = local_mcp_url(&endpoint.url)
            .ok_or_else(|| self.not_running("expected a local HTTP MCP endpoint"))?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let config =
            StreamableHttpClientTransportConfig::with_uri(url).auth_header(endpoint.token.clone());
        let transport = StreamableHttpClientTransport::with_client(client, config);
        let service = match tokio::time::timeout(CONNECT_WITHIN, ().serve(transport)).await {
            Ok(Ok(service)) => service,
            Ok(Err(error)) => {
                let text = error.to_string();
                return Err(
                    if text.contains("401") || text.to_lowercase().contains("unauthorized") {
                        format!(
                        "Backtracking Agent refused this connection's token ({text}). Try again; \
                         if it keeps happening, switch Backtracking Agent off and on in Options \
                         > Add-ons."
                    )
                    } else {
                        self.not_running(&format!("could not connect to {}: {text}", endpoint.url))
                    },
                );
            }
            Err(_) => return Err(self.not_running(&format!("{} did not answer", endpoint.url))),
        };
        let peer = service.peer().clone();
        *slot = Some(Connection { endpoint, service });
        Ok(peer)
    }

    async fn drop_connection(&self) {
        if let Some(old) = self.connection.lock().await.take() {
            let _ = old.service.cancel().await;
        }
    }

    async fn forward(&self, request: CallToolRequestParams) -> CallToolResponse {
        let name = request.name.to_string();
        let attempts = if READ_ONLY.contains(&name.as_str()) {
            2
        } else {
            1
        };
        let mut last = String::new();
        for _ in 0..attempts {
            let peer = match self.peer().await {
                Ok(peer) => peer,
                Err(text) => return tools::tool_error(text).into(),
            };
            match peer.call_tool(request.clone()).await {
                Ok(result) => return result.into(),
                Err(error) => {
                    last = error.to_string();
                    self.drop_connection().await;
                }
            }
        }
        let text = if attempts > 1 {
            format!("The connection to Backtracking was lost ({last}). Try again.")
        } else {
            format!(
                "The connection to Backtracking was lost while it worked on {name} ({last}). Call \
                 get_state to see whether it happened before trying again."
            )
        };
        tools::tool_error(text).into()
    }
}

impl ServerHandler for Forwarder {
    fn get_info(&self) -> ServerConfig {
        tools::server_config()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(AgentServer::definitions()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if !AgentServer::definitions()
            .iter()
            .any(|tool| tool.name == request.name)
        {
            return Err(McpError::invalid_params(
                format!("Backtracking has no tool named {}", request.name),
                None,
            ));
        }
        Ok(self.forward(request).await)
    }
}

#[cfg(test)]
mod tests {
    use super::local_mcp_url;

    #[test]
    fn rc1_stdio_accepts_only_numeric_loopback_mcp() {
        for url in ["http://127.0.0.1:1234/mcp", "http://[::1]:80/mcp"] {
            assert_eq!(local_mcp_url(url).as_deref(), Some(url));
        }
        for url in [
            "http://192.0.2.1:80/mcp",
            "http://localhost:80/mcp",
            "https://127.0.0.1:443/mcp",
            "http://127.0.0.1:0/mcp",
            "http://127.0.0.1:80/other",
            "http://user@127.0.0.1:80/mcp",
            "http://127.0.0.1:80/mcp?q=1",
        ] {
            assert!(local_mcp_url(url).is_none());
        }
    }
}
