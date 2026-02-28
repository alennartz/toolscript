use std::collections::HashMap;
use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use rmcp::service::{Peer, RoleClient, RunningService, ServiceError};
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use tokio::sync::Mutex;

use crate::config::McpServerConfigEntry;

/// Resolved configuration for connecting to an upstream MCP server.
///
/// This is produced by resolving and validating a [`McpServerConfigEntry`].
#[derive(Debug, Clone)]
pub enum McpServerResolvedConfig {
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    Http {
        url: String,
    },
}

impl McpServerResolvedConfig {
    /// Resolve from a validated config entry.
    ///
    /// Callers must ensure the entry has been validated via
    /// [`crate::config::validate_mcp_server_entry`] first.
    pub fn from_entry(entry: &McpServerConfigEntry) -> anyhow::Result<Self> {
        if let Some(cmd) = &entry.command {
            Ok(Self::Stdio {
                command: cmd.clone(),
                args: entry.args.clone().unwrap_or_default(),
                env: entry.env.clone().unwrap_or_default(),
            })
        } else if let Some(url) = &entry.url {
            Ok(Self::Http { url: url.clone() })
        } else {
            anyhow::bail!("config entry must have either 'command' or 'url'")
        }
    }
}

/// Type-erased wrapper around the transport-specific `RunningService`.
///
/// rmcp's `RunningService` is generic over the transport, so a stdio service
/// and an HTTP service are different concrete types. This enum lets us store
/// either variant behind a single handle while exposing the common
/// `Peer<RoleClient>` for making requests.
enum ServiceHandle {
    Stdio(RunningService<RoleClient, ()>),
    Http(RunningService<RoleClient, ()>),
}

impl ServiceHandle {
    #[allow(clippy::match_same_arms)]
    fn peer(&self) -> &Peer<RoleClient> {
        match self {
            Self::Stdio(s) => s.peer(),
            Self::Http(s) => s.peer(),
        }
    }

    #[allow(clippy::match_same_arms)]
    fn is_closed(&self) -> bool {
        match self {
            Self::Stdio(s) => s.is_closed(),
            Self::Http(s) => s.is_closed(),
        }
    }

    async fn close(&mut self) {
        match self {
            Self::Stdio(s) | Self::Http(s) => {
                let _ = s.close().await;
            }
        }
    }
}

/// Per-server state: the live service handle plus the config needed for reconnect.
struct McpClientHandle {
    service: ServiceHandle,
    config: McpServerResolvedConfig,
}

/// Manages connections to upstream MCP servers.
///
/// Each server is identified by its config-file name (e.g. `"filesystem"`, `"remote"`).
/// The manager provides `list_tools` and `call_tool` methods that delegate to the
/// appropriate server, and `call_tool` includes automatic single-retry reconnect logic.
pub struct McpClientManager {
    clients: HashMap<String, Arc<Mutex<McpClientHandle>>>,
}

impl std::fmt::Debug for McpClientManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClientManager")
            .field("servers", &self.clients.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Connect to a single upstream MCP server and return its service handle.
async fn connect_one(config: &McpServerResolvedConfig) -> anyhow::Result<ServiceHandle> {
    match config {
        McpServerResolvedConfig::Stdio { command, args, env } => {
            let env_clone = env.clone();
            let args_clone = args.clone();
            let transport = TokioChildProcess::new(
                tokio::process::Command::new(command).configure(move |cmd| {
                    cmd.args(&args_clone);
                    for (k, v) in &env_clone {
                        cmd.env(k, v);
                    }
                }),
            )
            .map_err(|e| anyhow::anyhow!("failed to spawn MCP server process: {e}"))?;
            let service = ()
                .serve(transport)
                .await
                .map_err(|e| anyhow::anyhow!("failed to initialize MCP client (stdio): {e}"))?;
            Ok(ServiceHandle::Stdio(service))
        }
        McpServerResolvedConfig::Http { url } => {
            let transport = StreamableHttpClientTransport::from_uri(url.as_str());
            let service = ()
                .serve(transport)
                .await
                .map_err(|e| anyhow::anyhow!("failed to initialize MCP client (http): {e}"))?;
            Ok(ServiceHandle::Http(service))
        }
    }
}

impl McpClientManager {
    /// Connect to all configured upstream MCP servers concurrently.
    ///
    /// If a server fails to connect, logs a warning and continues with the
    /// remaining servers. Returns a manager with whatever connections succeeded
    /// (possibly empty).
    pub async fn connect_all(
        configs: HashMap<String, McpServerResolvedConfig>,
    ) -> anyhow::Result<Self> {
        let futures: Vec<_> = configs
            .into_iter()
            .map(|(name, config)| async move {
                match connect_one(&config).await {
                    Ok(handle) => Some((
                        name,
                        Arc::new(Mutex::new(McpClientHandle {
                            service: handle,
                            config,
                        })),
                    )),
                    Err(e) => {
                        eprintln!("MCP: failed to connect to '{name}', skipping: {e}");
                        None
                    }
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;
        let clients: HashMap<_, _> = results.into_iter().flatten().collect();
        Ok(Self { clients })
    }

    /// Create an empty manager (no upstream servers).
    pub fn empty() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    /// Create a manager from a pre-connected `RunningService`.
    ///
    /// Useful for testing with in-process MCP servers (e.g. via `tokio::io::duplex`).
    /// Reconnection is not supported for services created this way.
    pub fn from_running_service(name: &str, service: RunningService<RoleClient, ()>) -> Self {
        let handle = McpClientHandle {
            service: ServiceHandle::Stdio(service),
            config: McpServerResolvedConfig::Stdio {
                command: String::new(),
                args: vec![],
                env: HashMap::new(),
            },
        };
        let mut clients = HashMap::new();
        clients.insert(name.to_string(), Arc::new(Mutex::new(handle)));
        Self { clients }
    }

    /// Returns the names of all connected servers.
    pub fn server_names(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }

    /// Returns true if no upstream servers are configured.
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// List tools from a specific upstream MCP server.
    pub async fn list_tools(&self, server: &str) -> anyhow::Result<Vec<Tool>> {
        let handle = self
            .clients
            .get(server)
            .ok_or_else(|| anyhow::anyhow!("unknown MCP server: '{server}'"))?;
        let tools = handle
            .lock()
            .await
            .service
            .peer()
            .list_all_tools()
            .await
            .map_err(|e| anyhow::anyhow!("failed to list tools from '{server}': {e}"))?;
        Ok(tools)
    }

    /// List tools from all connected upstream MCP servers concurrently.
    ///
    /// Returns a map from server name to its tools.
    pub async fn list_all_tools(&self) -> anyhow::Result<HashMap<String, Vec<Tool>>> {
        let futures: Vec<_> = self
            .clients
            .keys()
            .map(|name| async move {
                let tools = self.list_tools(name).await?;
                Ok::<_, anyhow::Error>((name.clone(), tools))
            })
            .collect();

        let results = futures::future::join_all(futures).await;
        results.into_iter().collect()
    }

    /// Call a tool on a specific upstream MCP server.
    ///
    /// On transport failure, attempts one reconnect then retries the call.
    /// If the reconnect or retry also fails, returns the original error.
    pub async fn call_tool(
        &self,
        server: &str,
        tool_name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> anyhow::Result<CallToolResult> {
        let handle = self
            .clients
            .get(server)
            .ok_or_else(|| anyhow::anyhow!("unknown MCP server: '{server}'"))?;

        let mut guard = handle.lock().await;

        // First attempt
        let params = CallToolRequestParams {
            meta: None,
            name: tool_name.to_string().into(),
            arguments: arguments.clone(),
            task: None,
        };
        let first_result = guard.service.peer().call_tool(params).await;

        match first_result {
            Ok(result) => Ok(result),
            Err(e) if is_transport_error(&e) || guard.service.is_closed() => {
                // Transport failure: attempt reconnect
                eprintln!("MCP: reconnecting to '{server}' after transport error...");
                let config = guard.config.clone();
                guard.service.close().await;

                match connect_one(&config).await {
                    Ok(new_handle) => {
                        eprintln!("MCP: reconnected to '{server}'");
                        guard.service = new_handle;
                        // Retry the call
                        let retry_params = CallToolRequestParams {
                            meta: None,
                            name: tool_name.to_string().into(),
                            arguments,
                            task: None,
                        };
                        guard
                            .service
                            .peer()
                            .call_tool(retry_params)
                            .await
                            .map_err(|retry_err| {
                                anyhow::anyhow!(
                                    "call_tool retry to '{server}' failed after reconnect: {retry_err}"
                                )
                            })
                    }
                    Err(reconnect_err) => {
                        eprintln!("MCP: reconnect to '{server}' failed: {reconnect_err}");
                        Err(anyhow::anyhow!(
                            "reconnect to '{server}' failed (original error: {e}): {reconnect_err}"
                        ))
                    }
                }
            }
            Err(e) => Err(anyhow::anyhow!("call_tool to '{server}' failed: {e}")),
        }
    }

    /// Gracefully shut down all connections.
    pub async fn close_all(&self) {
        for handle in self.clients.values() {
            handle.lock().await.service.close().await;
        }
    }
}

/// Determine whether a `ServiceError` indicates a transport-level failure
/// (as opposed to a normal MCP error response).
const fn is_transport_error(e: &ServiceError) -> bool {
    matches!(
        e,
        ServiceError::TransportSend(_) | ServiceError::TransportClosed
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn resolve_stdio_config() {
        let entry = McpServerConfigEntry {
            command: Some("npx".to_string()),
            args: Some(vec!["-y".to_string(), "server-fs".to_string()]),
            env: Some(HashMap::from([("FOO".to_string(), "bar".to_string())])),
            url: None,
        };
        let resolved = McpServerResolvedConfig::from_entry(&entry).unwrap();
        match resolved {
            McpServerResolvedConfig::Stdio { command, args, env } => {
                assert_eq!(command, "npx");
                assert_eq!(args, vec!["-y", "server-fs"]);
                assert_eq!(env.get("FOO").unwrap(), "bar");
            }
            McpServerResolvedConfig::Http { .. } => panic!("expected Stdio config"),
        }
    }

    #[test]
    fn resolve_http_config() {
        let entry = McpServerConfigEntry {
            command: None,
            args: None,
            env: None,
            url: Some("https://mcp.example.com/mcp".to_string()),
        };
        let resolved = McpServerResolvedConfig::from_entry(&entry).unwrap();
        match resolved {
            McpServerResolvedConfig::Http { url } => {
                assert_eq!(url, "https://mcp.example.com/mcp");
            }
            McpServerResolvedConfig::Stdio { .. } => panic!("expected Http config"),
        }
    }

    #[test]
    fn resolve_stdio_defaults_empty_vecs() {
        let entry = McpServerConfigEntry {
            command: Some("my-server".to_string()),
            args: None,
            env: None,
            url: None,
        };
        let resolved = McpServerResolvedConfig::from_entry(&entry).unwrap();
        match resolved {
            McpServerResolvedConfig::Stdio { args, env, .. } => {
                assert!(args.is_empty());
                assert!(env.is_empty());
            }
            McpServerResolvedConfig::Http { .. } => panic!("expected Stdio config"),
        }
    }

    #[test]
    fn resolve_entry_no_command_no_url_errors() {
        let entry = McpServerConfigEntry {
            command: None,
            args: None,
            env: None,
            url: None,
        };
        assert!(McpServerResolvedConfig::from_entry(&entry).is_err());
    }

    #[test]
    fn empty_manager_has_no_servers() {
        let manager = McpClientManager::empty();
        assert!(manager.is_empty());
        assert!(manager.server_names().is_empty());
    }

    #[tokio::test]
    async fn connect_all_empty_succeeds() {
        let manager = McpClientManager::connect_all(HashMap::new()).await.unwrap();
        assert!(manager.is_empty());
    }

    #[tokio::test]
    async fn list_tools_unknown_server_errors() {
        let manager = McpClientManager::empty();
        let result = manager.list_tools("nonexistent").await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("nonexistent"),
            "error should name the server: {msg}"
        );
    }

    #[tokio::test]
    async fn call_tool_unknown_server_errors() {
        let manager = McpClientManager::empty();
        let result = manager.call_tool("nonexistent", "some_tool", None).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("nonexistent"),
            "error should name the server: {msg}"
        );
    }

    #[test]
    fn is_transport_error_classification() {
        assert!(is_transport_error(&ServiceError::TransportClosed));
        // McpError is not a transport error
        assert!(!is_transport_error(&ServiceError::UnexpectedResponse));
    }

    #[tokio::test]
    async fn connect_all_bad_command_warns_and_continues() {
        let mut configs = HashMap::new();
        configs.insert(
            "bad".to_string(),
            McpServerResolvedConfig::Stdio {
                command: "/nonexistent/binary/that/does/not/exist".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        );
        // Should succeed (with warning to stderr) but have no clients
        let manager = McpClientManager::connect_all(configs).await.unwrap();
        assert!(manager.is_empty());
    }

    #[tokio::test]
    async fn connect_all_partial_failure_continues() {
        let mut configs = HashMap::new();
        configs.insert(
            "bad".to_string(),
            McpServerResolvedConfig::Stdio {
                command: "/nonexistent".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        );
        configs.insert(
            "also_bad".to_string(),
            McpServerResolvedConfig::Http {
                url: "http://127.0.0.1:1/nonexistent".to_string(),
            },
        );
        // Both fail, but connect_all should succeed with 0 clients
        let manager = McpClientManager::connect_all(configs).await.unwrap();
        assert!(manager.is_empty());
    }
}
