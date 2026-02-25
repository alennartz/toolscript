use std::collections::HashMap;
use std::path::Path;

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
    #[serde(default)]
    pub frozen_params: Option<HashMap<String, String>>,
}

/// Output configuration for `file.save()` in scripts.
#[derive(Debug, Clone, Deserialize)]
pub struct OutputConfig {
    pub dir: Option<String>,
    pub max_bytes: Option<u64>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CodeMcpConfig {
    pub apis: HashMap<String, ConfigApiEntry>,
    #[serde(default)]
    pub frozen_params: Option<HashMap<String, String>>,
    #[serde(default)]
    pub output: Option<OutputConfig>,
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
pub fn load_config(path: &Path) -> anyhow::Result<CodeMcpConfig> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read config file {}: {e}", path.display()))?;
    let config: CodeMcpConfig = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse config file {}: {e}", path.display()))?;
    Ok(config)
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
pub fn resolve_config_auth(config: &CodeMcpConfig) -> anyhow::Result<AuthCredentialsMap> {
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
    fn test_resolve_config_auth_direct() {
        let mut apis = HashMap::new();
        apis.insert(
            "petstore".to_string(),
            ConfigApiEntry {
                spec: "petstore.yaml".to_string(),
                auth: Some(ConfigAuth::Direct("sk-direct-token".to_string())),
                auth_env: None,
                frozen_params: None,
            },
        );
        let config = CodeMcpConfig {
            apis,
            frozen_params: None,
            output: None,
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
                frozen_params: None,
            },
        );
        let config = CodeMcpConfig {
            apis,
            frozen_params: None,
            output: None,
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
                frozen_params: None,
            },
        );
        let config = CodeMcpConfig {
            apis,
            frozen_params: None,
            output: None,
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
    fn test_load_config_with_output() {
        let toml_content = r#"
[output]
dir = "/tmp/my-output"
max_bytes = 1048576
enabled = false

[apis.petstore]
spec = "petstore.yaml"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        let output = config.output.unwrap();
        assert_eq!(output.dir.as_deref(), Some("/tmp/my-output"));
        assert_eq!(output.max_bytes, Some(1048576));
        assert_eq!(output.enabled, Some(false));
    }

    #[test]
    fn test_load_config_without_output() {
        let toml_content = r#"
[apis.petstore]
spec = "petstore.yaml"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_content.as_bytes()).unwrap();

        let config = load_config(tmpfile.path()).unwrap();
        assert!(config.output.is_none());
    }
}
