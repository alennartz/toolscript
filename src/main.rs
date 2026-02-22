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
use code_mcp::server::auth::McpAuthConfig;

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
            auth_authority,
            auth_audience,
            auth_jwks_uri,
        } => {
            let auth_config = build_auth_config(auth_authority, auth_audience, auth_jwks_uri)?;
            let manifest = load_manifest(&dir)?;
            serve(manifest, &transport, port, auth_config).await
        }
        Command::Run {
            specs,
            transport,
            port,
            auth_authority,
            auth_audience,
            auth_jwks_uri,
        } => {
            let auth_config = build_auth_config(auth_authority, auth_audience, auth_jwks_uri)?;
            let tmpdir = tempfile::tempdir()?;
            generate(&specs, tmpdir.path()).await?;
            let manifest = load_manifest(tmpdir.path())?;
            serve(manifest, &transport, port, auth_config).await
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

/// Validate auth CLI flags: authority and audience must both be set or both omitted.
fn build_auth_config(
    auth_authority: Option<String>,
    auth_audience: Option<String>,
    auth_jwks_uri: Option<String>,
) -> anyhow::Result<Option<McpAuthConfig>> {
    match (auth_authority, auth_audience) {
        (Some(authority), Some(audience)) => Ok(Some(McpAuthConfig {
            authority,
            audience,
            jwks_uri_override: auth_jwks_uri,
        })),
        (None, None) => Ok(None),
        _ => {
            anyhow::bail!("--auth-authority and --auth-audience must both be set (or both omitted)")
        }
    }
}

/// Create a `CodeMcpServer` from a manifest and serve it with the given transport.
async fn serve(
    manifest: Manifest,
    transport: &str,
    port: u16,
    auth_config: Option<McpAuthConfig>,
) -> anyhow::Result<()> {
    let handler = Arc::new(HttpHandler::new());
    let auth = load_auth_from_env(&manifest);
    let config = ExecutorConfig::default();
    let server = CodeMcpServer::new(manifest, handler, auth, config);

    match transport {
        "stdio" => serve_stdio(server).await,
        "sse" | "http" => serve_http(server, port, auth_config).await,
        other => anyhow::bail!("Unknown transport: '{other}'. Use 'stdio' or 'sse'."),
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
async fn serve_http(
    server: CodeMcpServer,
    port: u16,
    auth_config: Option<McpAuthConfig>,
) -> anyhow::Result<()> {
    use code_mcp::server::auth::{JwtValidator, auth_middleware};
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
            Arc::default(),
            config,
        );

    let app = if let Some(auth_config) = auth_config {
        let validator = Arc::new(JwtValidator::new(auth_config.clone()));
        let auth_state = (validator, auth_config.clone());

        let well_known_json = serde_json::json!({
            "resource": auth_config.audience,
            "authorization_servers": [auth_config.authority],
            "bearer_methods_supported": ["header"],
            "resource_documentation": "https://github.com/alenna/code-mcp",
        });

        axum::Router::new()
            .nest_service("/mcp", service)
            .route_layer(axum::middleware::from_fn_with_state(
                auth_state,
                auth_middleware,
            ))
            .route(
                "/.well-known/oauth-protected-resource",
                axum::routing::get(move || async move { axum::Json(well_known_json) }),
            )
    } else {
        axum::Router::new().nest_service("/mcp", service)
    };

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("MCP server listening on http://{addr}/mcp");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            ct.cancel();
        })
        .await?;

    Ok(())
}
