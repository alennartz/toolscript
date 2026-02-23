mod cli;

use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use cli::{Cli, Command};

use code_mcp::codegen::generate::generate;
use code_mcp::codegen::manifest::Manifest;
use code_mcp::config::{
    CodeMcpConfig, SpecInput, load_config, parse_auth_arg, parse_spec_arg, resolve_cli_auth,
    resolve_config_auth,
};
use code_mcp::runtime::executor::ExecutorConfig;
use code_mcp::runtime::http::{AuthCredentialsMap, HttpHandler};
use code_mcp::server::CodeMcpServer;
use code_mcp::server::auth::McpAuthConfig;

/// Bundled arguments for the `serve` function to avoid `clippy::too_many_arguments`.
struct ServeArgs {
    manifest: Manifest,
    transport: String,
    port: u16,
    mcp_auth: Option<McpAuthConfig>,
    auth: AuthCredentialsMap,
    timeout: u64,
    memory_limit: usize,
    max_api_calls: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Generate {
            specs,
            output,
            config,
        } => {
            let spec_inputs = resolve_spec_inputs(&specs, config.as_deref())?;
            generate(&spec_inputs, &output).await?;
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
            api_auth,
            timeout,
            memory_limit,
            max_api_calls,
        } => {
            let mcp_auth =
                build_mcp_auth_config(auth_authority, auth_audience, auth_jwks_uri)?;
            let manifest = load_manifest(&dir)?;
            let api_names: Vec<String> = manifest.apis.iter().map(|a| a.name.clone()).collect();
            let auth_args: Vec<_> = api_auth
                .iter()
                .map(|a| parse_auth_arg(a))
                .collect::<Result<_, _>>()?;
            let auth = resolve_cli_auth(&auth_args, &api_names)?;
            warn_missing_auth(&manifest, &auth);
            serve(ServeArgs {
                manifest,
                transport,
                port,
                mcp_auth,
                auth,
                timeout,
                memory_limit,
                max_api_calls,
            })
            .await
        }
        Command::Run {
            specs,
            config,
            api_auth,
            transport,
            port,
            auth_authority,
            auth_audience,
            auth_jwks_uri,
            timeout,
            memory_limit,
            max_api_calls,
        } => {
            let mcp_auth =
                build_mcp_auth_config(auth_authority, auth_audience, auth_jwks_uri)?;
            let (spec_inputs, config_obj) = resolve_run_inputs(&specs, config.as_deref())?;
            let tmpdir = tempfile::tempdir()?;
            generate(&spec_inputs, tmpdir.path()).await?;
            let manifest = load_manifest(tmpdir.path())?;
            let api_names: Vec<String> = manifest.apis.iter().map(|a| a.name.clone()).collect();
            let auth = if !api_auth.is_empty() {
                let auth_args: Vec<_> = api_auth
                    .iter()
                    .map(|a| parse_auth_arg(a))
                    .collect::<Result<_, _>>()?;
                resolve_cli_auth(&auth_args, &api_names)?
            } else if let Some(ref cfg) = config_obj {
                resolve_config_auth(cfg)?
            } else {
                AuthCredentialsMap::new()
            };
            warn_missing_auth(&manifest, &auth);
            serve(ServeArgs {
                manifest,
                transport,
                port,
                mcp_auth,
                auth,
                timeout,
                memory_limit,
                max_api_calls,
            })
            .await
        }
    }
}

/// Resolve spec inputs for the Generate command from either positional args or config file.
fn resolve_spec_inputs(
    specs: &[String],
    config_path: Option<&Path>,
) -> anyhow::Result<Vec<SpecInput>> {
    if let Some(path) = config_path {
        if !specs.is_empty() {
            anyhow::bail!("cannot use --config with positional spec arguments");
        }
        let config = load_config(path)?;
        return Ok(config
            .apis
            .iter()
            .map(|(name, entry)| SpecInput {
                name: Some(name.clone()),
                source: entry.spec.clone(),
            })
            .collect());
    }
    if specs.is_empty() {
        anyhow::bail!("no specs provided. Pass spec paths/URLs as arguments or use --config");
    }
    Ok(specs.iter().map(|s| parse_spec_arg(s)).collect())
}

/// Resolve spec inputs for the Run command. Also returns the config object for auth resolution.
/// Supports auto-discovery of `code-mcp.toml` when no specs or config are provided.
fn resolve_run_inputs(
    specs: &[String],
    config_path: Option<&Path>,
) -> anyhow::Result<(Vec<SpecInput>, Option<CodeMcpConfig>)> {
    if let Some(path) = config_path {
        if !specs.is_empty() {
            anyhow::bail!("cannot use --config with positional spec arguments");
        }
        let config = load_config(path)?;
        let inputs: Vec<SpecInput> = config
            .apis
            .iter()
            .map(|(name, entry)| SpecInput {
                name: Some(name.clone()),
                source: entry.spec.clone(),
            })
            .collect();
        return Ok((inputs, Some(config)));
    }
    if specs.is_empty() {
        // Auto-discover code-mcp.toml
        let default_path = Path::new("code-mcp.toml");
        if default_path.exists() {
            let config = load_config(default_path)?;
            let inputs: Vec<SpecInput> = config
                .apis
                .iter()
                .map(|(name, entry)| SpecInput {
                    name: Some(name.clone()),
                    source: entry.spec.clone(),
                })
                .collect();
            return Ok((inputs, Some(config)));
        }
        anyhow::bail!(
            "no specs provided. Pass spec paths/URLs, use --config, or create code-mcp.toml"
        );
    }
    Ok((specs.iter().map(|s| parse_spec_arg(s)).collect(), None))
}

/// Warn about APIs that declare auth in their spec but have no credentials configured.
fn warn_missing_auth(manifest: &Manifest, auth: &AuthCredentialsMap) {
    for api in &manifest.apis {
        if api.auth.is_some() && !auth.contains_key(&api.name) {
            eprintln!(
                "warning: {}: spec declares auth but no credentials configured. \
                 API calls will likely fail with 401.",
                api.name
            );
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

/// Validate MCP auth CLI flags: authority and audience must both be set or both omitted.
fn build_mcp_auth_config(
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
async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let handler = Arc::new(HttpHandler::new());
    let config = ExecutorConfig {
        timeout_ms: args.timeout * 1000,
        memory_limit: Some(args.memory_limit * 1024 * 1024),
        max_api_calls: Some(args.max_api_calls),
    };
    let server = CodeMcpServer::new(args.manifest, handler, args.auth, config);

    match args.transport.as_str() {
        "stdio" => serve_stdio(server).await,
        "sse" | "http" => serve_http(server, args.port, args.mcp_auth).await,
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
