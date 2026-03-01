mod cli;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use cli::{Cli, Command};

use toolscript::codegen::generate::generate;
use toolscript::codegen::luau_types::{extract_schema_defs, json_schema_to_params};
use toolscript::codegen::manifest::{Manifest, McpServerEntry, McpToolDef};
use toolscript::config::{
    McpServerConfigEntry, SpecInput, ToolScriptConfig, load_config, parse_auth_arg, parse_mcp_arg,
    parse_spec_arg, resolve_cli_auth, resolve_config_auth, validate_mcp_server_entry,
};
use toolscript::runtime::executor::{ExecutorConfig, OutputConfig};
use toolscript::runtime::http::{AuthCredentialsMap, HttpHandler};
use toolscript::runtime::mcp_client::{McpClientManager, McpServerResolvedConfig};
use toolscript::server::ToolScriptServer;
use toolscript::server::auth::McpAuthConfig;

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
    output_config: Option<OutputConfig>,
    mcp_client: Arc<McpClientManager>,
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Generate {
            specs,
            output,
            config,
        } => {
            let (spec_inputs, config_obj) = resolve_spec_inputs(&specs, config.as_deref())?;
            let (global_frozen, per_api_frozen) = extract_frozen_params(config_obj.as_ref());
            generate(&spec_inputs, &output, &global_frozen, &per_api_frozen).await?;
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
            output_dir,
            mcp_servers: cli_mcp,
        } => {
            let mcp_auth = build_mcp_auth_config(auth_authority, auth_audience, auth_jwks_uri)?;

            // Resolve MCP configs (Serve has no TOML config, only CLI --mcp flags)
            let mcp_configs = resolve_mcp_configs(None, &cli_mcp)?;
            let (mcp_client, mcp_server_entries) = discover_mcp_tools(mcp_configs).await?;

            let mut manifest = load_manifest(&dir)?;
            manifest.mcp_servers = mcp_server_entries;

            if manifest.apis.is_empty() && manifest.mcp_servers.is_empty() {
                anyhow::bail!(
                    "no APIs or MCP servers configured. \
                     Add [apis] or [mcp_servers] to toolscript.toml, or pass specs/--mcp flags"
                );
            }

            let api_names: Vec<String> = manifest.apis.iter().map(|a| a.name.clone()).collect();
            let auth_args: Vec<_> = api_auth
                .iter()
                .map(|a| parse_auth_arg(a))
                .collect::<Result<_, _>>()?;
            let auth = resolve_cli_auth(&auth_args, &api_names)?;
            warn_missing_auth(&manifest, &auth);
            let output_config = resolve_output_config(
                output_dir.as_deref(),
                None, // no TOML config for bare serve
                mcp_auth.is_some(),
            );
            serve(ServeArgs {
                manifest,
                transport,
                port,
                mcp_auth,
                auth,
                timeout,
                memory_limit,
                max_api_calls,
                output_config,
                mcp_client,
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
            output_dir,
            mcp_servers: cli_mcp,
        } => {
            let mcp_auth = build_mcp_auth_config(auth_authority, auth_audience, auth_jwks_uri)?;

            // Resolve spec inputs. When no explicit specs or --config are given,
            // auto-discover toolscript.toml. This allows TOML files with only
            // [mcp_servers] (no [apis]) to work. If no TOML either, fall back
            // to MCP-only mode when CLI --mcp flags are present.
            let (spec_inputs, config_obj) = if specs.is_empty() && config.is_none() {
                let default_path = Path::new("toolscript.toml");
                if default_path.exists() {
                    let cfg = load_config(default_path)?;
                    let inputs: Vec<SpecInput> = cfg
                        .apis
                        .iter()
                        .map(|(name, entry)| SpecInput {
                            name: Some(name.clone()),
                            source: entry.spec.clone(),
                        })
                        .collect();
                    (inputs, Some(cfg))
                } else if !cli_mcp.is_empty() {
                    // No TOML, but CLI --mcp flags → MCP-only mode
                    (vec![], None)
                } else {
                    anyhow::bail!(
                        "no specs provided. Pass spec paths/URLs, use --config, or create toolscript.toml"
                    );
                }
            } else {
                resolve_run_inputs(&specs, config.as_deref())?
            };

            // Resolve MCP configs: merge TOML [mcp_servers] with CLI --mcp flags
            let mcp_configs = resolve_mcp_configs(config_obj.as_ref(), &cli_mcp)?;
            let (mcp_client, mcp_server_entries) = discover_mcp_tools(mcp_configs).await?;

            // Build manifest: generate from specs if we have any, otherwise create empty
            let manifest = if spec_inputs.is_empty() {
                Manifest {
                    apis: vec![],
                    functions: vec![],
                    schemas: vec![],
                    mcp_servers: mcp_server_entries,
                }
            } else {
                let tmpdir = tempfile::tempdir()?;
                let (global_frozen, per_api_frozen) = extract_frozen_params(config_obj.as_ref());
                generate(&spec_inputs, tmpdir.path(), &global_frozen, &per_api_frozen).await?;
                let mut m = load_manifest(tmpdir.path())?;
                m.mcp_servers = mcp_server_entries;
                m
            };

            if manifest.apis.is_empty() && manifest.mcp_servers.is_empty() {
                anyhow::bail!(
                    "no APIs or MCP servers configured. \
                     Add [apis] or [mcp_servers] to toolscript.toml, or pass specs/--mcp flags"
                );
            }

            let api_names: Vec<String> = manifest.apis.iter().map(|a| a.name.clone()).collect();
            // Start with config auth, then layer CLI --auth on top (CLI wins per-key)
            let mut auth = if let Some(ref cfg) = config_obj {
                resolve_config_auth(cfg)?
            } else {
                AuthCredentialsMap::new()
            };
            if !api_auth.is_empty() {
                let auth_args: Vec<_> = api_auth
                    .iter()
                    .map(|a| parse_auth_arg(a))
                    .collect::<Result<_, _>>()?;
                let cli_auth = resolve_cli_auth(&auth_args, &api_names)?;
                auth.extend(cli_auth);
            }
            warn_missing_auth(&manifest, &auth);
            let output_config = resolve_output_config(
                output_dir.as_deref(),
                config_obj.as_ref(),
                mcp_auth.is_some(),
            );
            serve(ServeArgs {
                manifest,
                transport,
                port,
                mcp_auth,
                auth,
                timeout,
                memory_limit,
                max_api_calls,
                output_config,
                mcp_client,
            })
            .await
        }
    }
}

/// Resolve spec inputs for the Generate command from either positional args or config file.
fn resolve_spec_inputs(
    specs: &[String],
    config_path: Option<&Path>,
) -> anyhow::Result<(Vec<SpecInput>, Option<ToolScriptConfig>)> {
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
        anyhow::bail!("no specs provided. Pass spec paths/URLs as arguments or use --config");
    }
    Ok((specs.iter().map(|s| parse_spec_arg(s)).collect(), None))
}

/// Resolve spec inputs for the Run command. Also returns the config object for auth resolution.
/// Supports auto-discovery of `toolscript.toml` when no specs or config are provided.
fn resolve_run_inputs(
    specs: &[String],
    config_path: Option<&Path>,
) -> anyhow::Result<(Vec<SpecInput>, Option<ToolScriptConfig>)> {
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
        // Auto-discover toolscript.toml
        let default_path = Path::new("toolscript.toml");
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
            "no specs provided. Pass spec paths/URLs, use --config, or create toolscript.toml"
        );
    }
    Ok((specs.iter().map(|s| parse_spec_arg(s)).collect(), None))
}

/// Extract global and per-API frozen params from a config object (if present).
fn extract_frozen_params(
    config: Option<&ToolScriptConfig>,
) -> (
    HashMap<String, String>,
    HashMap<String, HashMap<String, String>>,
) {
    let Some(config) = config else {
        return (HashMap::new(), HashMap::new());
    };
    let global = config.frozen_params.clone().unwrap_or_default();
    let per_api: HashMap<String, HashMap<String, String>> = config
        .apis
        .iter()
        .filter_map(|(name, entry)| {
            entry
                .frozen_params
                .as_ref()
                .map(|fp| (name.clone(), fp.clone()))
        })
        .collect();
    (global, per_api)
}

/// Build the resolved output config from CLI flags, TOML config, and mode.
///
/// In local mode (not hosted), file output is enabled by default with a sensible
/// directory and size limit. In hosted mode, it is disabled unless explicitly
/// enabled in the TOML config or overridden via CLI `--output-dir`.
fn resolve_output_config(
    cli_output_dir: Option<&str>,
    config: Option<&ToolScriptConfig>,
    is_hosted: bool,
) -> Option<OutputConfig> {
    // If hosted mode and no explicit CLI override, disable
    // (unless config explicitly enables it)
    if is_hosted && cli_output_dir.is_none() {
        let explicitly_enabled = config
            .and_then(|c| c.output.as_ref())
            .and_then(|o| o.enabled)
            .unwrap_or(false);
        if !explicitly_enabled {
            return None;
        }
    }

    // Check if explicitly disabled in config (and no CLI override)
    if cli_output_dir.is_none()
        && config
            .and_then(|c| c.output.as_ref())
            .and_then(|o| o.enabled)
            == Some(false)
    {
        return None;
    }

    let dir = cli_output_dir
        .map(PathBuf::from)
        .or_else(|| {
            config
                .and_then(|c| c.output.as_ref())
                .and_then(|o| o.dir.as_ref())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("./toolscript-output"));

    let max_bytes = config
        .and_then(|c| c.output.as_ref())
        .and_then(|o| o.max_bytes)
        .unwrap_or(50 * 1024 * 1024);

    Some(OutputConfig { dir, max_bytes })
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

/// Merge MCP server config from TOML `[mcp_servers]` and CLI `--mcp` flags, validate, and resolve.
///
/// CLI entries override TOML entries with the same name.
fn resolve_mcp_configs(
    config: Option<&ToolScriptConfig>,
    cli_mcp: &[String],
) -> anyhow::Result<HashMap<String, McpServerResolvedConfig>> {
    let mut entries: HashMap<String, McpServerConfigEntry> = HashMap::new();

    // Start with config file entries
    if let Some(cfg) = config
        && let Some(ref mcp) = cfg.mcp_servers
    {
        entries.extend(mcp.clone());
    }

    // Layer CLI --mcp on top (CLI wins)
    for arg in cli_mcp {
        let (name, entry) = parse_mcp_arg(arg)?;
        entries.insert(name, entry);
    }

    // Validate all entries
    for (name, entry) in &entries {
        validate_mcp_server_entry(name, entry)?;
    }

    // Resolve to McpServerResolvedConfig
    let mut resolved = HashMap::new();
    for (name, entry) in entries {
        resolved.insert(name, McpServerResolvedConfig::from_entry(&entry)?);
    }

    Ok(resolved)
}

/// Connect to upstream MCP servers, discover their tools, and build manifest entries.
async fn discover_mcp_tools(
    configs: HashMap<String, McpServerResolvedConfig>,
) -> anyhow::Result<(Arc<McpClientManager>, Vec<McpServerEntry>)> {
    if configs.is_empty() {
        return Ok((Arc::new(McpClientManager::empty()), vec![]));
    }

    let client = McpClientManager::connect_all(configs).await?;
    let all_tools = client.list_all_tools().await?;

    let mut servers = Vec::new();
    for (server_name, tools) in all_tools {
        let mut tool_defs = Vec::new();
        for tool in tools {
            let input_schema = serde_json::Value::Object(tool.input_schema.as_ref().clone());

            let params = json_schema_to_params(&input_schema);
            let schemas = extract_schema_defs(&input_schema);

            tool_defs.push(McpToolDef {
                name: tool.name.to_string(),
                server: server_name.clone(),
                description: tool.description.map(|d| d.to_string()),
                params,
                schemas,
                output_schemas: vec![],
            });
        }

        servers.push(McpServerEntry {
            name: server_name,
            description: None,
            tools: tool_defs,
        });
    }

    let client = Arc::new(client);
    Ok((client, servers))
}

/// Create a `ToolScriptServer` from a manifest and serve it with the given transport.
async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let handler = Arc::new(HttpHandler::new());
    let config = ExecutorConfig {
        timeout_ms: args.timeout * 1000,
        memory_limit: Some(args.memory_limit * 1024 * 1024),
        max_api_calls: Some(args.max_api_calls),
    };
    let mcp_client = args.mcp_client;
    let server = ToolScriptServer::new(
        args.manifest,
        handler,
        args.auth,
        config,
        args.output_config,
        mcp_client.clone(),
    );

    match args.transport.as_str() {
        "stdio" => serve_stdio(server, mcp_client).await,
        "sse" | "http" => serve_http(server, args.port, args.mcp_auth, mcp_client).await,
        other => anyhow::bail!("Unknown transport: '{other}'. Use 'stdio' or 'sse'."),
    }
}

/// Serve using stdio transport (JSON-RPC over stdin/stdout).
async fn serve_stdio(
    server: ToolScriptServer,
    mcp_client: Arc<McpClientManager>,
) -> anyhow::Result<()> {
    let router = server.into_router();
    let transport = rmcp::transport::io::stdio();
    let service = rmcp::serve_server(router, transport).await?;
    service.waiting().await?;
    mcp_client.close_all().await;
    Ok(())
}

/// Serve using streamable HTTP transport (SSE).
async fn serve_http(
    server: ToolScriptServer,
    port: u16,
    auth_config: Option<McpAuthConfig>,
    mcp_client: Arc<McpClientManager>,
) -> anyhow::Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };
    use tokio_util::sync::CancellationToken;
    use toolscript::server::auth::{JwtValidator, auth_middleware};

    let ct = CancellationToken::new();
    let config = StreamableHttpServerConfig {
        stateful_mode: true,
        cancellation_token: ct.child_token(),
        ..Default::default()
    };

    // Create an Arc<ToolScriptServer> and build tool routes that work with it.
    let server = Arc::new(server);

    let service: StreamableHttpService<
        rmcp::handler::server::router::Router<Arc<ToolScriptServer>>,
    > = StreamableHttpService::new(
        {
            let server = server.clone();
            move || {
                let router = rmcp::handler::server::router::Router::new(server.clone())
                    .with_tool(toolscript::server::tools::list_apis_tool_arc())
                    .with_tool(toolscript::server::tools::list_functions_tool_arc())
                    .with_tool(toolscript::server::tools::get_function_docs_tool_arc())
                    .with_tool(toolscript::server::tools::search_docs_tool_arc())
                    .with_tool(toolscript::server::tools::execute_script_tool_arc());
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
            "resource_documentation": "https://github.com/alenna/toolscript",
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

    mcp_client.close_all().await;
    Ok(())
}
