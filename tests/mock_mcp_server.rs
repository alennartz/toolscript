//! Mock MCP server for integration testing.
//!
//! Provides a simple in-process MCP server connected via `tokio::io::duplex()`
//! that can be used to test the full MCP client pipeline without spawning
//! external processes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::borrow::Cow;
use std::sync::Arc;

use futures::FutureExt;
use rmcp::ServerHandler;
use rmcp::handler::server::router::Router;
use rmcp::handler::server::router::tool::ToolRoute;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo, Tool};

use toolscript::codegen::luau_types::{extract_schema_defs, json_schema_to_params};
use toolscript::codegen::manifest::{McpServerEntry, McpToolDef};
use toolscript::runtime::mcp_client::McpClientManager;

// ---- Mock MCP Server ----

/// A minimal MCP server for testing with `echo` and `get_data` tools.
#[derive(Clone)]
struct MockMcpServer;

impl ServerHandler for MockMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: rmcp::model::ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "mock-mcp-server".to_string(),
                title: Some("Mock MCP Server".to_string()),
                version: "0.1.0".to_string(),
                ..Default::default()
            },
            instructions: None,
        }
    }
}

fn make_tool(name: &str, description: &str, schema: serde_json::Value) -> Tool {
    Tool::new(
        Cow::Owned(name.to_string()),
        Cow::Owned(description.to_string()),
        rmcp::model::object(schema),
    )
}

fn echo_tool() -> ToolRoute<MockMcpServer> {
    ToolRoute::new_dyn(
        make_tool(
            "echo",
            "Echo back the input text",
            serde_json::json!({
                "type": "object",
                "required": ["text"],
                "properties": {
                    "text": { "type": "string", "description": "Text to echo" }
                }
            }),
        ),
        |mut context: ToolCallContext<'_, MockMcpServer>| {
            let args = context.arguments.take().unwrap_or_default();
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            std::future::ready(Ok(CallToolResult::success(vec![Content::text(text)]))).boxed()
        },
    )
}

fn get_data_tool() -> ToolRoute<MockMcpServer> {
    ToolRoute::new_dyn(
        make_tool(
            "get_data",
            "Return structured JSON data",
            serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        ),
        |_context: ToolCallContext<'_, MockMcpServer>| {
            std::future::ready(Ok(CallToolResult::success(vec![Content::text(
                r#"{"key":"value"}"#,
            )])))
            .boxed()
        },
    )
}

impl MockMcpServer {
    fn into_router(self) -> Router<Self> {
        Router::new(self)
            .with_tool(echo_tool())
            .with_tool(get_data_tool())
    }
}

// ---- Test helper ----

/// Spawn a mock MCP server on a duplex stream and return a connected
/// `McpClientManager`.
///
/// The server runs in a background task. The returned manager has a single
/// server named `"mock"`.
async fn spawn_mock_server() -> (Arc<McpClientManager>, tokio::task::JoinHandle<()>) {
    let (client_stream, server_stream) = tokio::io::duplex(8192);

    let server = MockMcpServer;
    let router = server.into_router();

    let server_handle = tokio::spawn(async move {
        let (server_read, server_write) = tokio::io::split(server_stream);
        let service = rmcp::serve_server(router, (server_read, server_write))
            .await
            .expect("failed to start mock server");
        let _ = service.waiting().await;
    });

    // Give the server a moment to start
    tokio::task::yield_now().await;

    let (client_read, client_write) = tokio::io::split(client_stream);
    let client_service = rmcp::ServiceExt::serve((), (client_read, client_write))
        .await
        .expect("failed to connect mock client");

    let manager = McpClientManager::from_running_service("mock", client_service);

    (Arc::new(manager), server_handle)
}

// ---- Tests ----

#[tokio::test]
async fn test_mock_server_connect_and_list_tools() {
    let (manager, _handle) = spawn_mock_server().await;
    assert!(!manager.is_empty());
    assert!(manager.server_names().contains(&"mock".to_string()));

    let tools = manager.list_tools("mock").await.unwrap();
    let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert!(
        tool_names.contains(&"echo".to_string()),
        "Missing echo tool. Got: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"get_data".to_string()),
        "Missing get_data tool. Got: {tool_names:?}"
    );
}

#[tokio::test]
async fn test_mock_server_call_tool() {
    let (manager, _handle) = spawn_mock_server().await;
    let mut args = serde_json::Map::new();
    args.insert(
        "text".to_string(),
        serde_json::Value::String("hello world".to_string()),
    );
    let result = manager.call_tool("mock", "echo", Some(args)).await.unwrap();
    let text = result
        .content
        .iter()
        .find_map(|c| match c.raw {
            rmcp::model::RawContent::Text(ref t) => Some(t.text.clone()),
            _ => None,
        })
        .expect("no text content in result");
    assert_eq!(text, "hello world");
}

#[tokio::test]
async fn test_mock_server_call_tool_unknown_errors() {
    let (manager, _handle) = spawn_mock_server().await;
    let result = manager.call_tool("mock", "nonexistent_tool", None).await;
    assert!(result.is_err(), "Expected error for unknown tool");
}

#[tokio::test]
async fn test_mock_server_in_discovery_pipeline() {
    let (manager, _handle) = spawn_mock_server().await;
    let all_tools = manager.list_all_tools().await.unwrap();
    let mock_tools = all_tools.get("mock").unwrap();

    // Build McpServerEntry / McpToolDef shapes like discover_mcp_tools does
    let mut tool_defs = Vec::new();
    for tool in mock_tools {
        let input_schema = serde_json::Value::Object(tool.input_schema.as_ref().clone());
        let params = json_schema_to_params(&input_schema);
        let schemas = extract_schema_defs(&input_schema);

        tool_defs.push(McpToolDef {
            name: tool.name.to_string(),
            server: "mock".to_string(),
            description: tool
                .description
                .as_ref()
                .map(std::string::ToString::to_string),
            params,
            schemas,
            output_schemas: vec![],
        });
    }

    let server_entry = McpServerEntry {
        name: "mock".to_string(),
        description: None,
        tools: tool_defs,
    };

    // Verify the echo tool was properly discovered
    let echo_tool = server_entry
        .tools
        .iter()
        .find(|t| t.name == "echo")
        .expect("echo tool not found");
    assert_eq!(echo_tool.server, "mock");
    assert!(echo_tool.description.is_some());
    assert!(
        echo_tool.params.iter().any(|p| p.name == "text"),
        "Missing 'text' param. Got: {:?}",
        echo_tool.params
    );
}
