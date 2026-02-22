mod cli;

use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use cli::{Cli, Command};

use code_mcp::codegen::generate::generate;
use code_mcp::codegen::manifest::Manifest;
use code_mcp::runtime::executor::ExecutorConfig;
use code_mcp::runtime::http::{HttpHandler, load_auth_from_env};
use code_mcp::server::CodeMcpServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Generate { specs, output } => {
            generate(&specs, &output).await?;
            eprintln!("Generated output to {}", output.display());
            Ok(())
        }
        Command::Serve {
            dir,
            transport,
            port,
        } => {
            let manifest = load_manifest(&dir)?;
            serve(manifest, &transport, port).await
        }
        Command::Run {
            specs,
            transport,
            port,
        } => {
            let tmpdir = tempfile::tempdir()?;
            generate(&specs, tmpdir.path()).await?;
            let manifest = load_manifest(tmpdir.path())?;
            serve(manifest, &transport, port).await
        }
    }
}

/// Load a manifest from a directory's manifest.json file.
fn load_manifest(dir: &Path) -> anyhow::Result<Manifest> {
    let manifest_path = dir.join("manifest.json");
    let manifest_str = std::fs::read_to_string(&manifest_path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to read manifest from {}: {}",
            manifest_path.display(),
            e
        )
    })?;
    let manifest: Manifest = serde_json::from_str(&manifest_str)?;
    Ok(manifest)
}

/// Create a CodeMcpServer from a manifest and serve it with the given transport.
async fn serve(manifest: Manifest, transport: &str, port: u16) -> anyhow::Result<()> {
    let handler = Arc::new(HttpHandler::new());
    let auth = load_auth_from_env(&manifest);
    let config = ExecutorConfig::default();
    let server = CodeMcpServer::new(manifest, handler, auth, config);

    match transport {
        "stdio" => serve_stdio(server).await,
        "sse" | "http" => serve_http(server, port).await,
        other => anyhow::bail!("Unknown transport: '{}'. Use 'stdio' or 'sse'.", other),
    }
}

/// Serve using stdio transport (JSON-RPC over stdin/stdout).
async fn serve_stdio(server: CodeMcpServer) -> anyhow::Result<()> {
    let router = server.into_router();
    let transport = rmcp::transport::io::stdio();
    let service = rmcp::serve_server(router, transport).await?;
    service.waiting().await?;
    Ok(())
}

/// Serve using streamable HTTP transport (SSE).
async fn serve_http(server: CodeMcpServer, port: u16) -> anyhow::Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };
    use tokio_util::sync::CancellationToken;

    let ct = CancellationToken::new();
    let config = StreamableHttpServerConfig {
        stateful_mode: true,
        cancellation_token: ct.child_token(),
        ..Default::default()
    };

    // The service factory creates a new Router for each session.
    // We need to share the server data across sessions.
    // Since CodeMcpServer is not Clone, we need to use Arc and share state.
    // However, Router takes ownership. Instead, we pre-build what we need and
    // create a factory that builds fresh routers with shared state.
    //
    // Actually, looking at the rmcp API: StreamableHttpService takes a factory
    // Fn() -> Result<S, io::Error> where S: Service<RoleServer>.
    // Router<CodeMcpServer> implements Service<RoleServer>, but CodeMcpServer
    // is not Clone. We need to make the server data shareable.
    //
    // The simplest approach: Arc<CodeMcpServer> implements ServerHandler (rmcp
    // has impl_server_handler_for_wrapper!(Arc)), so Router<Arc<CodeMcpServer>>
    // should also work.

    // Create an Arc<CodeMcpServer> and build tool routes that work with it.
    let server = Arc::new(server);

    let service: StreamableHttpService<rmcp::handler::server::router::Router<Arc<CodeMcpServer>>> =
        StreamableHttpService::new(
            {
                let server = server.clone();
                move || {
                    let router = rmcp::handler::server::router::Router::new(server.clone())
                        .with_tool(code_mcp::server::tools::list_apis_tool_arc())
                        .with_tool(code_mcp::server::tools::list_functions_tool_arc())
                        .with_tool(code_mcp::server::tools::get_function_docs_tool_arc())
                        .with_tool(code_mcp::server::tools::search_docs_tool_arc())
                        .with_tool(code_mcp::server::tools::get_schema_tool_arc())
                        .with_tool(code_mcp::server::tools::execute_script_tool_arc());
                    Ok(router)
                }
            },
            Default::default(),
            config,
        );

    let app = axum::Router::new().nest_service("/mcp", service);
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("MCP server listening on http://{}/mcp", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to listen for ctrl+c");
            ct.cancel();
        })
        .await?;

    Ok(())
}
