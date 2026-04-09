use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::runtime::http::{AuthCredentials, AuthCredentialsMap};

/// A spec input with an optional user-chosen name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecInput {
    pub name: Option<String>,
    pub source: String,
}

/// Auth entry in a TOML config file. Uses serde untagged enum.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ConfigAuth {
    Direct(String),
    Basic {
        #[serde(rename = "type")]
        auth_type: String,
        username: String,
        password: String,
    },
    EnvRef {
        auth_env: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigApiEntry {
    pub spec: String,
    #[serde(default)]
    pub auth: Option<ConfigAuth>,
    #[serde(default)]
    pub auth_env: Option<String>,
    /// Shell command whose stdout provides the credential value.
    /// Executed via `sh -c`. Output is trimmed and parsed according to the
    /// API's `OpenAPI` security scheme (bearer token, API key, or `user:pass`
    /// for basic auth).
    #[serde(default)]
    pub auth_command: Option<String>,
    /// How often (in seconds) to refresh command-based credentials. Default: 2.
    #[serde(default)]
    pub auth_command_ttl: Option<u64>,
    /// How long (in seconds) to wait for a credential command before falling
    /// back to the cached value. Default: 5.
    #[serde(default)]
    pub auth_command_timeout: Option<u64>,
    /// Maximum age (in seconds) of a cached credential that can still be used
    /// as a fallback when a refresh hasn't completed. Default: 300.
    #[serde(default)]
    pub auth_command_max_age: Option<u64>,
    #[serde(default)]
    pub frozen_params: Option<HashMap<String, String>>,
}

/// I/O configuration for sandboxed file access in scripts.
#[derive(Debug, Clone, Deserialize)]
pub struct IoConfig {
    pub dir: Option<String>,
    pub max_bytes: Option<u64>,
    pub enabled: Option<bool>,
}

/// Configuration for an upstream MCP server (stdio or HTTP).
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfigEntry {
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolScriptConfig {
    #[serde(default)]
    pub apis: HashMap<String, ConfigApiEntry>,
    #[serde(default)]
    pub frozen_params: Option<HashMap<String, String>>,
    #[serde(default)]
    pub io: Option<IoConfig>,
    #[serde(default)]
    pub mcp_servers: Option<HashMap<String, McpServerConfigEntry>>,
}

/// Parses `name=source` or plain `source`.
///
/// Finds the first `=` and checks whether the part before it contains `://`.
/// If it does not, treat it as `name=source`. Otherwise, the whole string is a plain URL/path.
pub fn parse_spec_arg(arg: &str) -> SpecInput {
    arg.find('=').map_or_else(
        || SpecInput {
            name: None,
            source: arg.to_string(),
        },
        |eq_pos| {
            let before_eq = &arg[..eq_pos];
            // If the part before `=` contains `://`, the whole thing is a URL (e.g. https://...=)
            if before_eq.contains("://") {
                SpecInput {
                    name: None,
                    source: arg.to_string(),
                }
            } else {
                SpecInput {
                    name: Some(before_eq.to_string()),
                    source: arg[eq_pos + 1..].to_string(),
                }
            }
        },
    )
}

/// Parses `name:ENV_VAR` or plain `ENV_VAR`.
///
/// Splits on the first `:` to separate an optional name prefix from the env var name.
pub fn parse_auth_arg(arg: &str) -> anyhow::Result<(Option<String>, String)> {
    if arg.is_empty() {
        anyhow::bail!("--auth value cannot be empty");
    }
    arg.find(':').map_or_else(
        || Ok((None, arg.to_string())),
        |colon_pos| {
            let name = &arg[..colon_pos];
            let env_var = &arg[colon_pos + 1..];
            if name.is_empty() || env_var.is_empty() {
                anyhow::bail!("invalid --auth format '{arg}': expected NAME:ENV_VAR or ENV_VAR");
            }
            Ok((Some(name.to_string()), env_var.to_string()))
        },
    )
}

/// Read and parse a TOML config file.
pub fn load_config(path: &Path) -> anyhow::Result<ToolScriptConfig> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read config file {}: {e}", path.display()))?;
    let config: ToolScriptConfig = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse config file {}: {e}", path.display()))?;
    Ok(config)
}

/// Merge two configs with block-level semantics.
///
/// For map fields (`apis`, `mcp_servers`): overlay entries replace base entries per key,
/// base-only entries are preserved. For scalar fields (`frozen_params`, `io`): if the
/// overlay sets the field, it replaces the base value entirely.
pub fn merge_configs(base: ToolScriptConfig, overlay: ToolScriptConfig) -> ToolScriptConfig {
    let mut apis = base.apis;
    apis.extend(overlay.apis);

    let frozen_params = overlay.frozen_params.or(base.frozen_params);
    let io = overlay.io.or(base.io);

    let mcp_servers = match (base.mcp_servers, overlay.mcp_servers) {
        (Some(mut base_mcp), Some(overlay_mcp)) => {
            base_mcp.extend(overlay_mcp);
            Some(base_mcp)
        }
        (base_mcp, overlay_mcp) => overlay_mcp.or(base_mcp),
    };

    ToolScriptConfig {
        apis,
        frozen_params,
        io,
        mcp_servers,
    }
}

/// Load multiple config files and merge them in order (later files win).
pub fn load_and_merge_configs(paths: &[PathBuf]) -> anyhow::Result<ToolScriptConfig> {
    anyhow::ensure!(!paths.is_empty(), "no config paths provided");
    let mut result = load_config(&paths[0])?;
    for path in &paths[1..] {
        let overlay = load_config(path)?;
        result = merge_configs(result, overlay);
    }
    Ok(result)
}

/// Merge global and per-API frozen params. Per-API values override global.
pub fn merge_frozen_params<S: std::hash::BuildHasher>(
    global: Option<&HashMap<String, String, S>>,
    per_api: Option<&HashMap<String, String, S>>,
) -> HashMap<String, String> {
    let mut merged: HashMap<String, String> = global
        .map(|g| g.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    if let Some(api_params) = per_api {
        merged.extend(api_params.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    merged
}

/// Parse `name=command_or_url` CLI arg into an `McpServerConfigEntry`.
///
/// If the value starts with `http://` or `https://`, it's treated as a URL
/// (transport defaults to streamable-http). Otherwise, it's split on spaces
/// where the first token is the command and the rest are args.
pub fn parse_mcp_arg(arg: &str) -> anyhow::Result<(String, McpServerConfigEntry)> {
    let eq_pos = arg.find('=').ok_or_else(|| {
        anyhow::anyhow!("invalid --mcp format '{arg}': expected name=command_or_url")
    })?;
    let name = &arg[..eq_pos];
    let value = &arg[eq_pos + 1..];
    if name.is_empty() || value.is_empty() {
        anyhow::bail!("invalid --mcp format '{arg}': name and value must be non-empty");
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        Ok((
            name.to_string(),
            McpServerConfigEntry {
                command: None,
                args: None,
                env: None,
                url: Some(value.to_string()),
            },
        ))
    } else {
        let parts: Vec<&str> = value.split_whitespace().collect();
        let command = parts[0].to_string();
        let args = if parts.len() > 1 {
            Some(parts[1..].iter().map(|s| (*s).to_string()).collect())
        } else {
            None
        };
        Ok((
            name.to_string(),
            McpServerConfigEntry {
                command: Some(command),
                args,
                env: None,
                url: None,
            },
        ))
    }
}

/// Validate a single MCP server config entry.
///
/// Rules:
/// - Must set exactly one of `command` or `url` (not both, not neither).
/// - `args` and `env` are only valid with `command`, not `url`.
pub fn validate_mcp_server_entry(name: &str, entry: &McpServerConfigEntry) -> anyhow::Result<()> {
    match (&entry.command, &entry.url) {
        (Some(_), Some(_)) => {
            anyhow::bail!("mcp_servers.{name}: cannot set both 'command' and 'url'");
        }
        (None, None) => {
            anyhow::bail!("mcp_servers.{name}: must set either 'command' or 'url'");
        }
        (Some(_), None) => {
            // stdio mode: ok
        }
        (None, Some(_)) => {
            // HTTP mode: args and env are not valid
            if entry.args.is_some() {
                anyhow::bail!("mcp_servers.{name}: 'args' is only valid with 'command', not 'url'");
            }
            if entry.env.is_some() {
                anyhow::bail!("mcp_servers.{name}: 'env' is only valid with 'command', not 'url'");
            }
        }
    }
    Ok(())
}

/// Read env vars and build auth map from CLI `--auth` arguments.
///
/// Unnamed auth only works with exactly one spec. Unknown names are errors.
/// Stores values as `AuthCredentials::BearerToken`.
pub fn resolve_cli_auth(
    auth_args: &[(Option<String>, String)],
    api_names: &[String],
) -> anyhow::Result<AuthCredentialsMap> {
    let mut map = AuthCredentialsMap::new();

    for (name, env_var) in auth_args {
        let token = std::env::var(env_var)
            .map_err(|_| anyhow::anyhow!("environment variable '{env_var}' is not set"))?;

        let api_name = if let Some(n) = name {
            if !api_names.contains(n) {
                return Err(anyhow::anyhow!(
                    "auth name '{n}' does not match any known API (known: {api_names:?})"
                ));
            }
            n.clone()
        } else {
            if api_names.len() != 1 {
                return Err(anyhow::anyhow!(
                    "unnamed --auth requires exactly one spec, but found multiple APIs: {api_names:?}"
                ));
            }
            api_names[0].clone()
        };

        map.insert(api_name, AuthCredentials::BearerToken(token));
    }

    Ok(map)
}

/// Resolve auth credentials from a TOML config.
///
/// Direct strings become `BearerToken`, Basic becomes `Basic`, `EnvRef` reads the env var.
/// The `auth_env` field on `ConfigApiEntry` is an alternative to the `EnvRef` variant.
pub fn resolve_config_auth(config: &ToolScriptConfig) -> anyhow::Result<AuthCredentialsMap> {
    let mut map = AuthCredentialsMap::new();

    for (name, entry) in &config.apis {
        // The `auth_env` field on ConfigApiEntry is an alternative to the EnvRef variant
        if let Some(env_var) = &entry.auth_env {
            let token = std::env::var(env_var).map_err(|_| {
                anyhow::anyhow!(
                    "environment variable '{env_var}' (from auth_env for '{name}') is not set"
                )
            })?;
            map.insert(name.clone(), AuthCredentials::BearerToken(token));
            continue;
        }

        if let Some(auth) = &entry.auth {
            match auth {
                ConfigAuth::Direct(token) => {
                    map.insert(name.clone(), AuthCredentials::BearerToken(token.clone()));
                }
                ConfigAuth::Basic {
                    auth_type: _,
                    username,
                    password,
                } => {
                    map.insert(
                        name.clone(),
                        AuthCredentials::Basic {
                            username: username.clone(),
                            password: password.clone(),
                        },
                    );
                }
                ConfigAuth::EnvRef { auth_env } => {
                    let token = std::env::var(auth_env).map_err(|_| {
                        anyhow::anyhow!(
                            "environment variable '{auth_env}' (from auth.auth_env for '{name}') is not set"
                        )
                    })?;
                    map.insert(name.clone(), AuthCredentials::BearerToken(token));
                }
            }
        }
    }

    Ok(map)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, unsafe_code)]
    use super::*;
    use std::io::Write as _;

    #[test]
    fn test_parse_spec_arg_plain_path() {
        let result = parse_spec_arg("petstore.yaml");
        assert_eq!(
            result,
            SpecInput {
                name: None,
                source: "petstore.yaml".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_spec_arg_plain_url() {
        let result = parse_spec_arg("https://example.com/spec.json");
        assert_eq!(
            result,
            SpecInput {
                name: None,
                source: "https://example.com/spec.json".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_spec_arg_named() {
        let result = parse_spec_arg("petstore=petstore.yaml");
        assert_eq!(
            result,
            SpecInput {
                name: Some("petstore".to_string()),
                source: "petstore.yaml".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_spec_arg_named_url() {
        let result = parse_spec_arg("myapi=https://example.com/spec.json");
        assert_eq!(
            result,
            SpecInput {
                name: Some("myapi".to_string()),
                source: "https://example.com/spec.json".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_auth_arg_named() {
        let result = parse_auth_arg("petstore:MY_TOKEN").unwrap();
        assert_eq!(
            result,
            (Some("petstore".to_string()), "MY_TOKEN".to_string())
        );
    }

    #[test]
    fn test_parse_auth_arg_unnamed() {
        let result = parse_auth_arg("MY_TOKEN").unwrap();
        assert_eq!(result, (None, "MY_TOKEN".to_string()));
    }

    #[test]
    fn test_parse_auth_arg_empty_name_errors() {
        let result = parse_auth_arg(":MY_TOKEN");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_auth_arg_empty_env_errors() {
        let result = parse_auth_arg("petstore:");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_auth_arg_empty_errors() {
        let result = parse_auth_arg("");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_cli_auth_named() {
        // SAFETY: test-only env manipulation; tests run serially for env vars
        unsafe { std::env::set_var("TEST_CLI_AUTH_TOKEN", "secret123") };
        let auth_args = vec![(
            Some("petstore".to_string()),
            "TEST_CLI_AUTH_TOKEN".to_string(),
        )];
        let api_names = vec!["petstore".to_string()];
        let result = resolve_cli_auth(&auth_args, &api_names).unwrap();
        unsafe { std::env::remove_var("TEST_CLI_AUTH_TOKEN") };

        assert!(result.contains_key("petstore"));
        match &result["petstore"] {
            AuthCredentials::BearerToken(token) => assert_eq!(token, "secret123"),
            other => panic!("Expected BearerToken, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_cli_auth_unnamed_single_spec() {
        // SAFETY: test-only env manipulation; tests run serially for env vars
        unsafe { std::env::set_var("TEST_CLI_AUTH_UNNAMED", "token456") };
        let auth_args = vec![(None, "TEST_CLI_AUTH_UNNAMED".to_string())];
        let api_names = vec!["onlyapi".to_string()];
        let result = resolve_cli_auth(&auth_args, &api_names).unwrap();
        unsafe { std::env::remove_var("TEST_CLI_AUTH_UNNAMED") };

        assert!(result.contains_key("onlyapi"));
        match &result["onlyapi"] {
            AuthCredentials::BearerToken(token) => assert_eq!(token, "token456"),
            other => panic!("Expected BearerToken, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_cli_auth_unnamed_multiple_specs_errors() {
        // SAFETY: test-only env manipulation; tests run serially for env vars
        unsafe { std::env::set_var("TEST_CLI_AUTH_MULTI", "token789") };
        let auth_args = vec![(None, "TEST_CLI_AUTH_MULTI".to_string())];
        let api_names = vec!["api1".to_string(), "api2".to_string()];
        let result = resolve_cli_auth(&auth_args, &api_names);
        unsafe { std::env::remove_var("TEST_CLI_AUTH_MULTI") };

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("multiple"),
            "Error should mention multiple specs: {err_msg}"
        );
    }

    #[test]
    fn test_resolve_cli_auth_name_not_found_errors() {
        // SAFETY: test-only env manipulation; tests run serially for env vars
        unsafe { std::env::set_var("TEST_CLI_AUTH_NOTFOUND", "tokenxyz") };
        let auth_args = vec![(
            Some("nonexistent".to_string()),
            "TEST_CLI_AUTH_NOTFOUND".to_string(),
        )];
        let api_names = vec!["petstore".to_string()];
        let result = resolve_cli_auth(&auth_args, &api_names);
        unsafe { std::env::remove_var("TEST_CLI_AUTH_NOTFOUND") };

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("nonexistent"),
            "Error should mention unknown name: {err_msg}"
        );
    }

    #[test]
    fn test_load_config_basic() {
        let toml_content = r#"
[apis.petstore]
spec = "petstore.yaml"
auth = "sk-my-token"

[apis.github]
spec = "https://raw.githubusercontent.com/github/rest-api-description/main/descriptions/api.github.com/api.github.com.yaml"
auth = "ghp_xxxxxxxxxxxx"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        assert_eq!(config.apis.len(), 2);
        assert_eq!(config.apis["petstore"].spec, "petstore.yaml");
        assert!(config.apis.contains_key("github"));

        match &config.apis["petstore"].auth {
            Some(ConfigAuth::Direct(s)) => assert_eq!(s, "sk-my-token"),
            other => panic!("Expected Direct auth, got {other:?}"),
        }
    }

    #[test]
    fn test_load_config_basic_auth() {
        let toml_content = r#"
[apis.myapi]
spec = "myapi.yaml"

[apis.myapi.auth]
type = "basic"
username = "user1"
password = "pass1"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        match &config.apis["myapi"].auth {
            Some(ConfigAuth::Basic {
                auth_type,
                username,
                password,
            }) => {
                assert_eq!(auth_type, "basic");
                assert_eq!(username, "user1");
                assert_eq!(password, "pass1");
            }
            other => panic!("Expected Basic auth, got {other:?}"),
        }
    }

    #[test]
    fn test_load_config_auth_env() {
        let toml_content = r#"
[apis.myapi]
spec = "myapi.yaml"
auth_env = "MY_API_TOKEN"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        assert_eq!(
            config.apis["myapi"].auth_env.as_deref(),
            Some("MY_API_TOKEN")
        );
    }

    #[test]
    fn test_load_config_auth_command() {
        let toml_content = r#"
[apis.myapi]
spec = "myapi.yaml"
auth_command = "vault read -field=token secret/myapi"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        assert_eq!(
            config.apis["myapi"].auth_command.as_deref(),
            Some("vault read -field=token secret/myapi")
        );
        // No static auth should be set
        assert!(config.apis["myapi"].auth.is_none());
        assert!(config.apis["myapi"].auth_env.is_none());
    }

    #[test]
    fn test_load_config_auth_command_with_timeouts() {
        let toml_content = r#"
[apis.myapi]
spec = "myapi.yaml"
auth_command = "get-token.sh"
auth_command_ttl = 10
auth_command_timeout = 15
auth_command_max_age = 600
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        let api = &config.apis["myapi"];
        assert_eq!(api.auth_command.as_deref(), Some("get-token.sh"));
        assert_eq!(api.auth_command_ttl, Some(10));
        assert_eq!(api.auth_command_timeout, Some(15));
        assert_eq!(api.auth_command_max_age, Some(600));
    }

    #[test]
    fn test_load_config_auth_command_defaults_none() {
        let toml_content = r#"
[apis.myapi]
spec = "myapi.yaml"
auth_command = "get-token.sh"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        let api = &config.apis["myapi"];
        assert!(api.auth_command_ttl.is_none());
        assert!(api.auth_command_timeout.is_none());
        assert!(api.auth_command_max_age.is_none());
    }

    #[test]
    fn test_resolve_config_auth_direct() {
        let mut apis = HashMap::new();
        apis.insert(
            "petstore".to_string(),
            ConfigApiEntry {
                spec: "petstore.yaml".to_string(),
                auth: Some(ConfigAuth::Direct("sk-direct-token".to_string())),
                auth_env: None,
                auth_command: None,
                auth_command_ttl: None,
                auth_command_timeout: None,
                auth_command_max_age: None,
                frozen_params: None,
            },
        );
        let config = ToolScriptConfig {
            apis,
            frozen_params: None,
            io: None,
            mcp_servers: None,
        };
        let result = resolve_config_auth(&config).unwrap();

        match &result["petstore"] {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "sk-direct-token"),
            other => panic!("Expected BearerToken, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_config_auth_basic() {
        let mut apis = HashMap::new();
        apis.insert(
            "myapi".to_string(),
            ConfigApiEntry {
                spec: "myapi.yaml".to_string(),
                auth: Some(ConfigAuth::Basic {
                    auth_type: "basic".to_string(),
                    username: "admin".to_string(),
                    password: "hunter2".to_string(),
                }),
                auth_env: None,
                auth_command: None,
                auth_command_ttl: None,
                auth_command_timeout: None,
                auth_command_max_age: None,
                frozen_params: None,
            },
        );
        let config = ToolScriptConfig {
            apis,
            frozen_params: None,
            io: None,
            mcp_servers: None,
        };
        let result = resolve_config_auth(&config).unwrap();

        match &result["myapi"] {
            AuthCredentials::Basic { username, password } => {
                assert_eq!(username, "admin");
                assert_eq!(password, "hunter2");
            }
            other => panic!("Expected Basic, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_config_auth_env_ref() {
        // SAFETY: test-only env manipulation; tests run serially for env vars
        unsafe { std::env::set_var("TEST_CONFIG_ENV_REF", "envtoken999") };
        let mut apis = HashMap::new();
        apis.insert(
            "myapi".to_string(),
            ConfigApiEntry {
                spec: "myapi.yaml".to_string(),
                auth: Some(ConfigAuth::EnvRef {
                    auth_env: "TEST_CONFIG_ENV_REF".to_string(),
                }),
                auth_env: None,
                auth_command: None,
                auth_command_ttl: None,
                auth_command_timeout: None,
                auth_command_max_age: None,
                frozen_params: None,
            },
        );
        let config = ToolScriptConfig {
            apis,
            frozen_params: None,
            io: None,
            mcp_servers: None,
        };
        let result = resolve_config_auth(&config).unwrap();
        unsafe { std::env::remove_var("TEST_CONFIG_ENV_REF") };

        match &result["myapi"] {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "envtoken999"),
            other => panic!("Expected BearerToken, got {other:?}"),
        }
    }

    #[test]
    fn test_load_config_with_frozen_params() {
        let toml_content = r#"
[frozen_params]
api_version = "v2"

[apis.petstore]
spec = "petstore.yaml"

[apis.petstore.frozen_params]
tenant_id = "abc-123"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        let global = config.frozen_params.as_ref().unwrap();
        assert_eq!(global.get("api_version").unwrap(), "v2");

        let api_frozen = config.apis["petstore"].frozen_params.as_ref().unwrap();
        assert_eq!(api_frozen.get("tenant_id").unwrap(), "abc-123");
    }

    #[test]
    fn test_load_config_without_frozen_params() {
        let toml_content = r#"
[apis.petstore]
spec = "petstore.yaml"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        assert!(config.frozen_params.is_none());
        assert!(config.apis["petstore"].frozen_params.is_none());
    }

    #[test]
    fn test_merge_frozen_params_precedence() {
        let mut global = HashMap::new();
        global.insert("api_version".to_string(), "v1".to_string());
        global.insert("tenant".to_string(), "default".to_string());

        let mut per_api = HashMap::new();
        per_api.insert("api_version".to_string(), "v2".to_string());

        let merged = merge_frozen_params(Some(&global), Some(&per_api));
        assert_eq!(merged.get("api_version").unwrap(), "v2"); // per-API wins
        assert_eq!(merged.get("tenant").unwrap(), "default"); // global preserved
    }

    #[test]
    fn test_load_config_with_io() {
        let toml_content = r#"
[io]
dir = "/tmp/my-output"
max_bytes = 1048576
enabled = false

[apis.petstore]
spec = "petstore.yaml"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        let io = config.io.unwrap();
        assert_eq!(io.dir.as_deref(), Some("/tmp/my-output"));
        assert_eq!(io.max_bytes, Some(1_048_576));
        assert_eq!(io.enabled, Some(false));
    }

    #[test]
    fn test_load_config_without_io() {
        let toml_content = r#"
[apis.petstore]
spec = "petstore.yaml"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        assert!(config.io.is_none());
    }

    #[test]
    fn test_load_config_with_mcp_servers() {
        let toml_content = r#"
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

[mcp_servers.remote]
url = "https://mcp.example.com/mcp"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        let mcp = config.mcp_servers.as_ref().unwrap();
        assert_eq!(mcp.len(), 2);

        let fs = &mcp["filesystem"];
        assert_eq!(fs.command.as_deref(), Some("npx"));
        assert_eq!(fs.args.as_ref().unwrap().len(), 3);
        assert!(fs.url.is_none());

        let remote = &mcp["remote"];
        assert_eq!(remote.url.as_deref(), Some("https://mcp.example.com/mcp"));
        assert!(remote.command.is_none());
    }

    #[test]
    fn test_load_config_mcp_only() {
        let toml_content = r#"
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        assert!(config.apis.is_empty());
        assert!(config.mcp_servers.as_ref().unwrap().len() == 1);
    }

    #[test]
    fn test_validate_mcp_server_config() {
        // command + url = error
        let entry = McpServerConfigEntry {
            command: Some("npx".to_string()),
            args: None,
            env: None,
            url: Some("https://example.com".to_string()),
        };
        assert!(validate_mcp_server_entry("test", &entry).is_err());

        // neither = error
        let entry = McpServerConfigEntry {
            command: None,
            args: None,
            env: None,
            url: None,
        };
        assert!(validate_mcp_server_entry("test", &entry).is_err());

        // args with url = error
        let entry = McpServerConfigEntry {
            command: None,
            args: Some(vec!["foo".to_string()]),
            env: None,
            url: Some("https://example.com".to_string()),
        };
        assert!(validate_mcp_server_entry("test", &entry).is_err());
    }

    #[test]
    fn test_parse_mcp_arg_command() {
        let (name, entry) =
            parse_mcp_arg("filesystem=npx -y @modelcontextprotocol/server-filesystem /tmp")
                .unwrap();
        assert_eq!(name, "filesystem");
        assert_eq!(entry.command.as_deref(), Some("npx"));
        assert_eq!(entry.args.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn test_parse_mcp_arg_url() {
        let (name, entry) = parse_mcp_arg("remote=https://mcp.example.com/mcp").unwrap();
        assert_eq!(name, "remote");
        assert_eq!(entry.url.as_deref(), Some("https://mcp.example.com/mcp"));
        assert!(entry.command.is_none());
    }

    #[test]
    fn test_parse_mcp_arg_invalid() {
        assert!(parse_mcp_arg("noequals").is_err());
        assert!(parse_mcp_arg("=value").is_err());
        assert!(parse_mcp_arg("name=").is_err());
    }

    #[test]
    fn test_merge_configs_apis() {
        let base = ToolScriptConfig {
            apis: HashMap::from([
                (
                    "petstore".to_string(),
                    ConfigApiEntry {
                        spec: "petstore-v1.yaml".to_string(),
                        auth: Some(ConfigAuth::Direct("old-token".to_string())),
                        auth_env: None,
                        auth_command: None,
                        auth_command_ttl: None,
                        auth_command_timeout: None,
                        auth_command_max_age: None,
                        frozen_params: None,
                    },
                ),
                (
                    "github".to_string(),
                    ConfigApiEntry {
                        spec: "github.yaml".to_string(),
                        auth: None,
                        auth_env: None,
                        auth_command: None,
                        auth_command_ttl: None,
                        auth_command_timeout: None,
                        auth_command_max_age: None,
                        frozen_params: None,
                    },
                ),
            ]),
            frozen_params: None,
            io: None,
            mcp_servers: None,
        };
        let overlay = ToolScriptConfig {
            apis: HashMap::from([(
                "petstore".to_string(),
                ConfigApiEntry {
                    spec: "petstore-v2.yaml".to_string(),
                    auth: None,
                    auth_env: None,
                    auth_command: None,
                    auth_command_ttl: None,
                    auth_command_timeout: None,
                    auth_command_max_age: None,
                    frozen_params: None,
                },
            )]),
            frozen_params: None,
            io: None,
            mcp_servers: None,
        };
        let merged = merge_configs(base, overlay);
        assert_eq!(merged.apis.len(), 2);
        // overlay petstore replaces base (block-level: auth is gone)
        assert_eq!(merged.apis["petstore"].spec, "petstore-v2.yaml");
        assert!(merged.apis["petstore"].auth.is_none());
        // github survives from base
        assert_eq!(merged.apis["github"].spec, "github.yaml");
    }

    #[test]
    fn test_merge_configs_io_block_level() {
        let base = ToolScriptConfig {
            apis: HashMap::new(),
            frozen_params: None,
            io: Some(IoConfig {
                dir: Some("/tmp/base".to_string()),
                max_bytes: Some(1024),
                enabled: Some(true),
            }),
            mcp_servers: None,
        };
        let overlay = ToolScriptConfig {
            apis: HashMap::new(),
            frozen_params: None,
            io: Some(IoConfig {
                dir: Some("/tmp/overlay".to_string()),
                max_bytes: None,
                enabled: None,
            }),
            mcp_servers: None,
        };
        let merged = merge_configs(base, overlay);
        let io = merged.io.unwrap();
        // Block-level: overlay replaces entire io block
        assert_eq!(io.dir.as_deref(), Some("/tmp/overlay"));
        assert!(io.max_bytes.is_none());
        assert!(io.enabled.is_none());
    }

    #[test]
    fn test_merge_configs_io_preserved_when_overlay_absent() {
        let base = ToolScriptConfig {
            apis: HashMap::new(),
            frozen_params: None,
            io: Some(IoConfig {
                dir: Some("/tmp/base".to_string()),
                max_bytes: Some(1024),
                enabled: Some(true),
            }),
            mcp_servers: None,
        };
        let overlay = ToolScriptConfig {
            apis: HashMap::new(),
            frozen_params: None,
            io: None,
            mcp_servers: None,
        };
        let merged = merge_configs(base, overlay);
        assert_eq!(merged.io.unwrap().dir.as_deref(), Some("/tmp/base"));
    }

    #[test]
    fn test_merge_configs_mcp_servers() {
        let base = ToolScriptConfig {
            apis: HashMap::new(),
            frozen_params: None,
            io: None,
            mcp_servers: Some(HashMap::from([(
                "filesystem".to_string(),
                McpServerConfigEntry {
                    command: Some("npx".to_string()),
                    args: Some(vec!["-y".to_string(), "server-fs".to_string()]),
                    env: None,
                    url: None,
                },
            )])),
        };
        let overlay = ToolScriptConfig {
            apis: HashMap::new(),
            frozen_params: None,
            io: None,
            mcp_servers: Some(HashMap::from([(
                "remote".to_string(),
                McpServerConfigEntry {
                    command: None,
                    args: None,
                    env: None,
                    url: Some("https://mcp.example.com".to_string()),
                },
            )])),
        };
        let merged = merge_configs(base, overlay);
        let mcp = merged.mcp_servers.unwrap();
        assert_eq!(mcp.len(), 2);
        assert!(mcp.contains_key("filesystem"));
        assert!(mcp.contains_key("remote"));
    }

    #[test]
    fn test_load_and_merge_configs() {
        let base_toml = r#"
[apis.petstore]
spec = "petstore.yaml"
auth = "base-token"

[io]
dir = "/tmp/base"
"#;
        let overlay_toml = r#"
[apis.petstore]
spec = "petstore-v2.yaml"

[apis.github]
spec = "github.yaml"
"#;
        let mut base_file = tempfile::NamedTempFile::new().unwrap();
        base_file.write_all(base_toml.as_bytes()).unwrap();
        let mut overlay_file = tempfile::NamedTempFile::new().unwrap();
        overlay_file.write_all(overlay_toml.as_bytes()).unwrap();

        let paths = vec![
            base_file.path().to_path_buf(),
            overlay_file.path().to_path_buf(),
        ];
        let config = load_and_merge_configs(&paths).unwrap();

        assert_eq!(config.apis.len(), 2);
        assert_eq!(config.apis["petstore"].spec, "petstore-v2.yaml");
        assert!(config.apis["petstore"].auth.is_none()); // block-level: auth gone
        assert_eq!(config.apis["github"].spec, "github.yaml");
        assert_eq!(config.io.unwrap().dir.as_deref(), Some("/tmp/base")); // preserved
    }
}
