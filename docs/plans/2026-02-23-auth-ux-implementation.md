# Auth UX Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the auto-derived env var auth convention with explicit CLI `--auth` flags and TOML config file support, with user-chosen API names.

**Architecture:** New `config.rs` module parses TOML config and CLI `--auth` flags into an `AuthCredentialsMap`. The `generate()` function accepts `SpecInput` structs (name + source) instead of bare strings. `load_auth_from_env()` is removed entirely. Resolution order: CLI `--auth` > config file > `_meta.auth` (runtime only).

**Tech Stack:** Rust, clap (derive), toml crate (new dependency), serde

---

### Task 1: Add `toml` dependency

**Files:**
- Modify: `Cargo.toml:19` (add after `tempfile`)

**Step 1: Add the dependency**

Add to `[dependencies]` in `Cargo.toml`:

```toml
toml = "0.8"
```

**Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add toml dependency for config file support"
```

---

### Task 2: Create `SpecInput` type and config module

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs:1` (add `pub mod config;`)
- Test: `src/config.rs` (inline tests)

**Step 1: Write failing tests for `SpecInput` parsing and TOML config**

Create `src/config.rs` with the following test module and just enough types to make the tests compile but fail:

```rust
use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::runtime::http::{AuthCredentials, AuthCredentialsMap};

/// A spec input with an optional user-chosen name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecInput {
    /// User-chosen API name. `None` means auto-derive from spec title.
    pub name: Option<String>,
    /// The spec source: file path or URL.
    pub source: String,
}

/// Parse a CLI spec argument. Accepts `name=source` or plain `source`.
pub fn parse_spec_arg(arg: &str) -> SpecInput {
    todo!()
}

/// Auth entry in a TOML config file.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ConfigAuth {
    /// Direct token/key value (bearer or api_key, determined by spec).
    Direct(String),
    /// Basic auth with username and password.
    Basic {
        #[serde(rename = "type")]
        auth_type: String,
        username: String,
        password: String,
    },
    /// Environment variable reference.
    EnvRef {
        auth_env: String,
    },
}

/// A single API entry in the TOML config.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigApiEntry {
    pub spec: String,
    #[serde(default)]
    pub auth: Option<ConfigAuth>,
    #[serde(default)]
    pub auth_env: Option<String>,
}

/// Top-level TOML config structure.
#[derive(Debug, Clone, Deserialize)]
pub struct CodeMcpConfig {
    pub apis: HashMap<String, ConfigApiEntry>,
}

/// Load and parse a TOML config file.
pub fn load_config(path: &Path) -> anyhow::Result<CodeMcpConfig> {
    todo!()
}

/// Parse a CLI `--auth` argument. Accepts `name:ENV_VAR` or `ENV_VAR`.
pub fn parse_auth_arg(arg: &str) -> anyhow::Result<(Option<String>, String)> {
    todo!()
}

/// Resolve auth credentials from CLI `--auth` args.
/// Returns a map from API name to credentials.
/// `api_name` is `None` when using the unnamed form (single-spec only).
pub fn resolve_cli_auth(
    auth_args: &[(Option<String>, String)],
    api_names: &[String],
) -> anyhow::Result<AuthCredentialsMap> {
    todo!()
}

/// Resolve auth credentials from a config file.
pub fn resolve_config_auth(config: &CodeMcpConfig) -> anyhow::Result<AuthCredentialsMap> {
    todo!()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, unsafe_code)]
    use super::*;

    // -- parse_spec_arg tests --

    #[test]
    fn test_parse_spec_arg_plain_path() {
        let input = parse_spec_arg("petstore.yaml");
        assert_eq!(input, SpecInput { name: None, source: "petstore.yaml".to_string() });
    }

    #[test]
    fn test_parse_spec_arg_plain_url() {
        let input = parse_spec_arg("https://example.com/spec.json");
        assert_eq!(input, SpecInput { name: None, source: "https://example.com/spec.json".to_string() });
    }

    #[test]
    fn test_parse_spec_arg_named() {
        let input = parse_spec_arg("petstore=petstore.yaml");
        assert_eq!(input, SpecInput { name: Some("petstore".to_string()), source: "petstore.yaml".to_string() });
    }

    #[test]
    fn test_parse_spec_arg_named_url() {
        let input = parse_spec_arg("myapi=https://example.com/spec.json");
        assert_eq!(input, SpecInput { name: Some("myapi".to_string()), source: "https://example.com/spec.json".to_string() });
    }

    // -- parse_auth_arg tests --

    #[test]
    fn test_parse_auth_arg_named() {
        let (name, env_var) = parse_auth_arg("petstore:MY_TOKEN").unwrap();
        assert_eq!(name, Some("petstore".to_string()));
        assert_eq!(env_var, "MY_TOKEN");
    }

    #[test]
    fn test_parse_auth_arg_unnamed() {
        let (name, env_var) = parse_auth_arg("MY_TOKEN").unwrap();
        assert_eq!(name, None);
        assert_eq!(env_var, "MY_TOKEN");
    }

    // -- resolve_cli_auth tests --

    #[test]
    fn test_resolve_cli_auth_named() {
        // SAFETY: test-only env manipulation
        unsafe { std::env::set_var("TEST_PET_TOKEN", "sk-pet-123") };
        let args = vec![(Some("petstore".to_string()), "TEST_PET_TOKEN".to_string())];
        let result = resolve_cli_auth(&args, &["petstore".to_string()]).unwrap();
        unsafe { std::env::remove_var("TEST_PET_TOKEN") };

        assert_eq!(result.len(), 1);
        match &result["petstore"] {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "sk-pet-123"),
            other => panic!("expected BearerToken, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_cli_auth_unnamed_single_spec() {
        // SAFETY: test-only env manipulation
        unsafe { std::env::set_var("TEST_SINGLE_TOKEN", "sk-single") };
        let args = vec![(None, "TEST_SINGLE_TOKEN".to_string())];
        let result = resolve_cli_auth(&args, &["myapi".to_string()]).unwrap();
        unsafe { std::env::remove_var("TEST_SINGLE_TOKEN") };

        assert_eq!(result.len(), 1);
        assert!(result.contains_key("myapi"));
    }

    #[test]
    fn test_resolve_cli_auth_unnamed_multiple_specs_errors() {
        let args = vec![(None, "SOME_TOKEN".to_string())];
        let result = resolve_cli_auth(&args, &["api1".to_string(), "api2".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_cli_auth_name_not_found_errors() {
        // SAFETY: test-only env manipulation
        unsafe { std::env::set_var("TEST_BAD_NAME", "val") };
        let args = vec![(Some("nonexistent".to_string()), "TEST_BAD_NAME".to_string())];
        let result = resolve_cli_auth(&args, &["petstore".to_string()]);
        unsafe { std::env::remove_var("TEST_BAD_NAME") };
        assert!(result.is_err());
    }

    // -- TOML config tests --

    #[test]
    fn test_load_config_basic() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("code-mcp.toml");
        std::fs::write(&config_path, r#"
[apis.petstore]
spec = "https://petstore.example.com/spec.json"
auth = "sk-my-token"

[apis.billing]
spec = "./billing.yaml"
auth = "key-billing-123"
"#).unwrap();

        let config = load_config(&config_path).unwrap();
        assert_eq!(config.apis.len(), 2);
        assert_eq!(config.apis["petstore"].spec, "https://petstore.example.com/spec.json");
        assert!(config.apis["petstore"].auth.is_some());
    }

    #[test]
    fn test_load_config_basic_auth() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("code-mcp.toml");
        std::fs::write(&config_path, r#"
[apis.legacy]
spec = "./legacy.yaml"

[apis.legacy.auth]
type = "basic"
username = "admin"
password = "secret"
"#).unwrap();

        let config = load_config(&config_path).unwrap();
        let legacy = &config.apis["legacy"];
        match legacy.auth.as_ref().unwrap() {
            ConfigAuth::Basic { username, password, .. } => {
                assert_eq!(username, "admin");
                assert_eq!(password, "secret");
            }
            other => panic!("expected Basic, got {:?}", other),
        }
    }

    #[test]
    fn test_load_config_auth_env() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("code-mcp.toml");
        std::fs::write(&config_path, r#"
[apis.stripe]
spec = "./stripe.yaml"
auth_env = "STRIPE_KEY"
"#).unwrap();

        let config = load_config(&config_path).unwrap();
        assert!(config.apis["stripe"].auth_env.is_some());
        assert_eq!(config.apis["stripe"].auth_env.as_deref(), Some("STRIPE_KEY"));
    }

    #[test]
    fn test_resolve_config_auth_direct() {
        let mut apis = HashMap::new();
        apis.insert("petstore".to_string(), ConfigApiEntry {
            spec: "spec.yaml".to_string(),
            auth: Some(ConfigAuth::Direct("sk-direct-token".to_string())),
            auth_env: None,
        });
        let config = CodeMcpConfig { apis };
        let result = resolve_config_auth(&config).unwrap();
        assert_eq!(result.len(), 1);
        match &result["petstore"] {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "sk-direct-token"),
            other => panic!("expected BearerToken, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_config_auth_basic() {
        let mut apis = HashMap::new();
        apis.insert("legacy".to_string(), ConfigApiEntry {
            spec: "spec.yaml".to_string(),
            auth: Some(ConfigAuth::Basic {
                auth_type: "basic".to_string(),
                username: "user".to_string(),
                password: "pass".to_string(),
            }),
            auth_env: None,
        });
        let config = CodeMcpConfig { apis };
        let result = resolve_config_auth(&config).unwrap();
        match &result["legacy"] {
            AuthCredentials::Basic { username, password } => {
                assert_eq!(username, "user");
                assert_eq!(password, "pass");
            }
            other => panic!("expected Basic, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_config_auth_env_ref() {
        // SAFETY: test-only env manipulation
        unsafe { std::env::set_var("TEST_STRIPE_KEY", "sk-from-env") };
        let mut apis = HashMap::new();
        apis.insert("stripe".to_string(), ConfigApiEntry {
            spec: "spec.yaml".to_string(),
            auth: None,
            auth_env: Some("TEST_STRIPE_KEY".to_string()),
        });
        let config = CodeMcpConfig { apis };
        let result = resolve_config_auth(&config).unwrap();
        unsafe { std::env::remove_var("TEST_STRIPE_KEY") };

        assert_eq!(result.len(), 1);
        match &result["stripe"] {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "sk-from-env"),
            other => panic!("expected BearerToken, got {:?}", other),
        }
    }
}
```

**Step 2: Add `pub mod config;` to `src/lib.rs`**

Add after line 1:

```rust
pub mod config;
```

**Step 3: Run tests to verify they fail**

Run: `cargo test --lib config`
Expected: FAIL — all tests panic with `todo!()`

**Step 4: Implement `parse_spec_arg`**

Replace the `todo!()` in `parse_spec_arg`:

```rust
pub fn parse_spec_arg(arg: &str) -> SpecInput {
    // URLs contain "://" which could be confused with name=https://...
    // Split only on the first "=" that appears before any "://"
    if let Some(eq_pos) = arg.find('=') {
        let before_eq = &arg[..eq_pos];
        // If there's no "://" before the "=", treat it as name=source
        if !before_eq.contains("://") {
            return SpecInput {
                name: Some(before_eq.to_string()),
                source: arg[eq_pos + 1..].to_string(),
            };
        }
    }
    SpecInput {
        name: None,
        source: arg.to_string(),
    }
}
```

**Step 5: Run parse_spec_arg tests**

Run: `cargo test --lib config::tests::test_parse_spec_arg`
Expected: all 4 `test_parse_spec_arg_*` tests PASS

**Step 6: Implement `parse_auth_arg`**

```rust
pub fn parse_auth_arg(arg: &str) -> anyhow::Result<(Option<String>, String)> {
    if let Some(colon_pos) = arg.find(':') {
        let name = &arg[..colon_pos];
        let env_var = &arg[colon_pos + 1..];
        if name.is_empty() || env_var.is_empty() {
            anyhow::bail!("invalid --auth format '{arg}': expected NAME:ENV_VAR or ENV_VAR");
        }
        Ok((Some(name.to_string()), env_var.to_string()))
    } else {
        if arg.is_empty() {
            anyhow::bail!("--auth value cannot be empty");
        }
        Ok((None, arg.to_string()))
    }
}
```

**Step 7: Run parse_auth_arg tests**

Run: `cargo test --lib config::tests::test_parse_auth_arg`
Expected: PASS

**Step 8: Implement `load_config`**

```rust
pub fn load_config(path: &Path) -> anyhow::Result<CodeMcpConfig> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read config file {}: {e}", path.display()))?;
    let config: CodeMcpConfig = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse config file {}: {e}", path.display()))?;
    Ok(config)
}
```

**Step 9: Run load_config tests**

Run: `cargo test --lib config::tests::test_load_config`
Expected: PASS

**Step 10: Implement `resolve_cli_auth`**

```rust
pub fn resolve_cli_auth(
    auth_args: &[(Option<String>, String)],
    api_names: &[String],
) -> anyhow::Result<AuthCredentialsMap> {
    let mut map = AuthCredentialsMap::new();
    for (name, env_var) in auth_args {
        let value = std::env::var(env_var)
            .map_err(|_| anyhow::anyhow!("environment variable '{env_var}' is not set"))?;
        let api_name = match name {
            Some(n) => {
                if !api_names.contains(n) {
                    anyhow::bail!(
                        "--auth {n}:{env_var} but no spec named '{n}' was loaded. Available: {}",
                        api_names.join(", ")
                    );
                }
                n.clone()
            }
            None => {
                if api_names.len() != 1 {
                    anyhow::bail!(
                        "--auth without a name prefix requires exactly one spec, but {} were loaded",
                        api_names.len()
                    );
                }
                api_names[0].clone()
            }
        };
        // Store as BearerToken by default; the inject_auth function in http.rs
        // will match it against the spec's AuthConfig to determine how to send it.
        map.insert(api_name, AuthCredentials::BearerToken(value));
    }
    Ok(map)
}
```

**Step 11: Run resolve_cli_auth tests**

Run: `cargo test --lib config::tests::test_resolve_cli_auth`
Expected: PASS

**Step 12: Implement `resolve_config_auth`**

```rust
pub fn resolve_config_auth(config: &CodeMcpConfig) -> anyhow::Result<AuthCredentialsMap> {
    let mut map = AuthCredentialsMap::new();
    for (api_name, entry) in &config.apis {
        if let Some(auth) = &entry.auth {
            match auth {
                ConfigAuth::Direct(token) => {
                    map.insert(api_name.clone(), AuthCredentials::BearerToken(token.clone()));
                }
                ConfigAuth::Basic { username, password, .. } => {
                    map.insert(api_name.clone(), AuthCredentials::Basic {
                        username: username.clone(),
                        password: password.clone(),
                    });
                }
                ConfigAuth::EnvRef { auth_env } => {
                    let value = std::env::var(auth_env).map_err(|_| {
                        anyhow::anyhow!("config for '{api_name}': env var '{auth_env}' is not set")
                    })?;
                    map.insert(api_name.clone(), AuthCredentials::BearerToken(value));
                }
            }
        } else if let Some(env_var) = &entry.auth_env {
            let value = std::env::var(env_var).map_err(|_| {
                anyhow::anyhow!("config for '{api_name}': env var '{env_var}' is not set")
            })?;
            map.insert(api_name.clone(), AuthCredentials::BearerToken(value));
        }
    }
    Ok(map)
}
```

**Step 13: Run all config tests**

Run: `cargo test --lib config`
Expected: all tests PASS

**Step 14: Commit**

```bash
git add src/config.rs src/lib.rs Cargo.toml Cargo.lock
git commit -m "feat: add config module with SpecInput, TOML config, and CLI auth parsing"
```

---

### Task 3: Update `generate()` to accept `SpecInput`

**Files:**
- Modify: `src/codegen/generate.rs:10-63`
- Modify: `tests/codegen_integration.rs`
- Modify: `tests/full_roundtrip.rs:13,129`

**Step 1: Update the `generate()` signature and implementation**

Change `generate()` in `src/codegen/generate.rs` to accept `&[SpecInput]` instead of `&[String]`:

```rust
use crate::config::SpecInput;

pub async fn generate(specs: &[SpecInput], output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;
    let sdk_dir = output_dir.join("sdk");
    std::fs::create_dir_all(&sdk_dir)?;

    let mut combined = Manifest {
        apis: vec![],
        functions: vec![],
        schemas: vec![],
    };

    for spec_input in specs {
        let spec = if spec_input.source.starts_with("http://") || spec_input.source.starts_with("https://") {
            parser::load_spec_from_url(&spec_input.source).await?
        } else {
            parser::load_spec_from_file(Path::new(&spec_input.source))?
        };
        let api_name = spec_input.name.clone().unwrap_or_else(|| derive_api_name(&spec));
        let manifest = parser::spec_to_manifest(&spec, &api_name)?;
        combined.apis.extend(manifest.apis);
        combined.functions.extend(manifest.functions);
        combined.schemas.extend(manifest.schemas);
    }

    // Write manifest.json
    let manifest_json = serde_json::to_string_pretty(&combined)?;
    std::fs::write(output_dir.join("manifest.json"), manifest_json)?;

    // Write annotation files
    let files = annotations::generate_annotation_files(&combined);
    for (filename, content) in files {
        std::fs::write(sdk_dir.join(filename), content)?;
    }

    Ok(())
}
```

**Step 2: Update inline test in `generate.rs`**

Update `test_generate_creates_output` to use `SpecInput`:

```rust
use crate::config::SpecInput;

#[tokio::test]
async fn test_generate_creates_output() {
    let output_dir = tempfile::tempdir().unwrap();
    generate(
        &[SpecInput { name: None, source: "testdata/petstore.yaml".to_string() }],
        output_dir.path(),
    )
    .await
    .unwrap();
    // ... rest unchanged
}
```

Also update `test_derive_api_name` and `test_derive_api_name_with_spaces` — these test the internal `derive_api_name` function and don't need changes.

**Step 3: Update `tests/codegen_integration.rs`**

```rust
use code_mcp::config::SpecInput;

#[tokio::test]
async fn test_generate_from_petstore() {
    let output_dir = tempfile::tempdir().unwrap();
    code_mcp::codegen::generate::generate(
        &[SpecInput { name: None, source: "testdata/petstore.yaml".to_string() }],
        output_dir.path(),
    )
    .await
    .unwrap();
    // ... rest unchanged
}
```

**Step 4: Update `tests/full_roundtrip.rs`**

Update both `generate()` calls:

```rust
use code_mcp::config::SpecInput;

// Line 13:
generate(
    &[SpecInput { name: None, source: "testdata/petstore.yaml".to_string() }],
    output_dir.path(),
)

// Line 129:
generate(
    &[SpecInput { name: None, source: "testdata/petstore.yaml".to_string() }],
    output_dir.path(),
)
```

**Step 5: Add test for explicit name override**

Add to `src/codegen/generate.rs` tests:

```rust
#[tokio::test]
async fn test_generate_with_explicit_name() {
    let output_dir = tempfile::tempdir().unwrap();
    generate(
        &[SpecInput { name: Some("mystore".to_string()), source: "testdata/petstore.yaml".to_string() }],
        output_dir.path(),
    )
    .await
    .unwrap();

    let manifest_str = std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap();
    let manifest: Manifest = serde_json::from_str(&manifest_str).unwrap();
    assert_eq!(manifest.apis[0].name, "mystore");
    // Functions should reference the explicit name
    for func in &manifest.functions {
        assert_eq!(func.api, "mystore");
    }
}
```

**Step 6: Run all tests**

Run: `cargo test`
Expected: all PASS

**Step 7: Commit**

```bash
git add src/codegen/generate.rs tests/codegen_integration.rs tests/full_roundtrip.rs
git commit -m "refactor: update generate() to accept SpecInput with optional name override"
```

---

### Task 4: Update CLI to add `--auth` and `--config` flags

**Files:**
- Modify: `src/cli.rs:1-82`

**Step 1: Update the `Run` and `Generate` variants**

Replace the `specs` field in `Run` and `Generate` to no longer require at least one arg when `--config` might be used. Add `--auth` and `--config` flags to `Run`. Add `--config` to `Generate`.

```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "code-mcp", about = "Generate MCP servers from OpenAPI specs")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Generate manifest and SDK annotations from `OpenAPI` specs
    Generate {
        /// Spec sources: `path`, `url`, or `name=path`/`name=url`
        specs: Vec<String>,
        /// Output directory
        #[arg(short, long, default_value = "./output")]
        output: PathBuf,
        /// Path to TOML config file
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Start MCP server from a generated directory
    Serve {
        /// Path to generated output directory
        #[arg(required = true)]
        dir: PathBuf,
        /// Transport type
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Port for SSE transport
        #[arg(long, default_value = "8080")]
        port: u16,
        /// OAuth authority URL for JWT validation (enables auth)
        #[arg(long, env = "MCP_AUTH_AUTHORITY")]
        auth_authority: Option<String>,
        /// Expected JWT audience (required if auth-authority is set)
        #[arg(long, env = "MCP_AUTH_AUDIENCE")]
        auth_audience: Option<String>,
        /// Explicit JWKS URI (optional, derived from authority via OIDC discovery if not set)
        #[arg(long, env = "MCP_AUTH_JWKS_URI")]
        auth_jwks_uri: Option<String>,
        /// Upstream API auth: `name:ENV_VAR` or `ENV_VAR` (for single-spec)
        #[arg(long = "auth")]
        api_auth: Vec<String>,
        /// Script execution timeout in seconds
        #[arg(long, default_value = "30")]
        timeout: u64,
        /// Luau VM memory limit in megabytes
        #[arg(long, default_value = "64")]
        memory_limit: usize,
        /// Maximum API calls per script execution
        #[arg(long, default_value = "100")]
        max_api_calls: usize,
    },
    /// Generate and serve in one step
    Run {
        /// Spec sources: `path`, `url`, or `name=path`/`name=url`
        specs: Vec<String>,
        /// Path to TOML config file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Upstream API auth: `name:ENV_VAR` or `ENV_VAR` (for single-spec)
        #[arg(long = "auth")]
        api_auth: Vec<String>,
        /// Transport type
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Port for SSE transport
        #[arg(long, default_value = "8080")]
        port: u16,
        /// OAuth authority URL for JWT validation (enables auth)
        #[arg(long, env = "MCP_AUTH_AUTHORITY")]
        auth_authority: Option<String>,
        /// Expected JWT audience (required if auth-authority is set)
        #[arg(long, env = "MCP_AUTH_AUDIENCE")]
        auth_audience: Option<String>,
        /// Explicit JWKS URI (optional, derived from authority via OIDC discovery if not set)
        #[arg(long, env = "MCP_AUTH_JWKS_URI")]
        auth_jwks_uri: Option<String>,
        /// Script execution timeout in seconds
        #[arg(long, default_value = "30")]
        timeout: u64,
        /// Luau VM memory limit in megabytes
        #[arg(long, default_value = "64")]
        memory_limit: usize,
        /// Maximum API calls per script execution
        #[arg(long, default_value = "100")]
        max_api_calls: usize,
    },
}
```

Note: `--auth` is the user-facing flag name, but clap field is `api_auth` to avoid collision with `auth_authority`/`auth_audience` fields. The `#[arg(long = "auth")]` maps the flag name.

**Step 2: Update CLI tests**

Replace the existing tests and add new ones:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use clap::Parser;

    #[test]
    fn test_run_with_spec() {
        let cli = Cli::parse_from(["code-mcp", "run", "spec.yaml"]);
        match cli.command {
            Command::Run { specs, config, api_auth, .. } => {
                assert_eq!(specs, vec!["spec.yaml"]);
                assert!(config.is_none());
                assert!(api_auth.is_empty());
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_run_with_config() {
        let cli = Cli::parse_from(["code-mcp", "run", "--config", "code-mcp.toml"]);
        match cli.command {
            Command::Run { specs, config, .. } => {
                assert!(specs.is_empty());
                assert_eq!(config.unwrap().to_str().unwrap(), "code-mcp.toml");
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_run_with_auth_flag() {
        let cli = Cli::parse_from([
            "code-mcp", "run", "petstore=spec.yaml",
            "--auth", "petstore:MY_TOKEN",
        ]);
        match cli.command {
            Command::Run { specs, api_auth, .. } => {
                assert_eq!(specs, vec!["petstore=spec.yaml"]);
                assert_eq!(api_auth, vec!["petstore:MY_TOKEN"]);
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_run_with_multiple_auth() {
        let cli = Cli::parse_from([
            "code-mcp", "run", "a=a.yaml", "b=b.yaml",
            "--auth", "a:TOKEN_A",
            "--auth", "b:TOKEN_B",
        ]);
        match cli.command {
            Command::Run { api_auth, .. } => {
                assert_eq!(api_auth.len(), 2);
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_run_defaults() {
        let cli = Cli::parse_from(["code-mcp", "run", "spec.yaml"]);
        match cli.command {
            Command::Run { timeout, memory_limit, max_api_calls, .. } => {
                assert_eq!(timeout, 30);
                assert_eq!(memory_limit, 64);
                assert_eq!(max_api_calls, 100);
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_serve_defaults() {
        let cli = Cli::parse_from(["code-mcp", "serve", "./output"]);
        match cli.command {
            Command::Serve { timeout, memory_limit, max_api_calls, .. } => {
                assert_eq!(timeout, 30);
                assert_eq!(memory_limit, 64);
                assert_eq!(max_api_calls, 100);
            }
            _ => panic!("expected Serve"),
        }
    }
}
```

**Step 3: Run tests**

Run: `cargo test --lib cli`
Expected: PASS

**Step 4: Commit**

```bash
git add src/cli.rs
git commit -m "feat: add --auth and --config CLI flags to run/generate/serve commands"
```

---

### Task 5: Rewrite `main.rs` to use new config/auth resolution

**Files:**
- Modify: `src/main.rs` (full rewrite of command handlers)

**Step 1: Rewrite main.rs**

Replace `src/main.rs` with the updated logic. Key changes:
- Parse `specs` through `parse_spec_arg`
- Support `--config` with auto-discovery of `code-mcp.toml`
- Resolve auth from CLI `--auth` or config file instead of `load_auth_from_env`
- Warn when a spec declares auth but no credentials are configured

```rust
mod cli;

use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use cli::{Cli, Command};

use code_mcp::codegen::generate::generate;
use code_mcp::codegen::manifest::Manifest;
use code_mcp::config::{
    self, CodeMcpConfig, SpecInput, load_config, parse_auth_arg, parse_spec_arg,
    resolve_cli_auth, resolve_config_auth,
};
use code_mcp::runtime::executor::ExecutorConfig;
use code_mcp::runtime::http::{AuthCredentialsMap, HttpHandler};
use code_mcp::server::CodeMcpServer;
use code_mcp::server::auth::McpAuthConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Generate { specs, output, config } => {
            let spec_inputs = resolve_spec_inputs(specs, config.as_deref())?;
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
            let mcp_auth = build_mcp_auth_config(auth_authority, auth_audience, auth_jwks_uri)?;
            let manifest = load_manifest(&dir)?;
            let api_names: Vec<String> = manifest.apis.iter().map(|a| a.name.clone()).collect();
            let auth_args: Vec<_> = api_auth.iter().map(|a| parse_auth_arg(a)).collect::<Result<_, _>>()?;
            let auth = resolve_cli_auth(&auth_args, &api_names)?;
            warn_missing_auth(&manifest, &auth);
            serve(manifest, &transport, port, mcp_auth, auth, timeout, memory_limit, max_api_calls).await
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
            let mcp_auth = build_mcp_auth_config(auth_authority, auth_audience, auth_jwks_uri)?;
            let (spec_inputs, config_obj) = resolve_run_inputs(specs, config.as_deref())?;

            // Generate
            let tmpdir = tempfile::tempdir()?;
            generate(&spec_inputs, tmpdir.path()).await?;
            let manifest = load_manifest(tmpdir.path())?;

            // Resolve auth
            let api_names: Vec<String> = manifest.apis.iter().map(|a| a.name.clone()).collect();
            let auth = if !api_auth.is_empty() {
                let auth_args: Vec<_> = api_auth.iter().map(|a| parse_auth_arg(a)).collect::<Result<_, _>>()?;
                resolve_cli_auth(&auth_args, &api_names)?
            } else if let Some(ref cfg) = config_obj {
                resolve_config_auth(cfg)?
            } else {
                AuthCredentialsMap::new()
            };

            warn_missing_auth(&manifest, &auth);
            serve(manifest, &transport, port, mcp_auth, auth, timeout, memory_limit, max_api_calls).await
        }
    }
}

/// Resolve spec inputs from CLI args and/or config file.
fn resolve_spec_inputs(specs: Vec<String>, config_path: Option<&Path>) -> anyhow::Result<Vec<SpecInput>> {
    if let Some(path) = config_path {
        if !specs.is_empty() {
            anyhow::bail!("cannot use --config with positional spec arguments");
        }
        let config = load_config(path)?;
        return Ok(config.apis.iter().map(|(name, entry)| SpecInput {
            name: Some(name.clone()),
            source: entry.spec.clone(),
        }).collect());
    }

    if specs.is_empty() {
        anyhow::bail!("no specs provided. Pass spec paths/URLs as arguments or use --config");
    }

    Ok(specs.iter().map(|s| parse_spec_arg(s)).collect())
}

/// Like `resolve_spec_inputs` but also returns the parsed config for auth resolution.
/// Supports auto-discovery of `code-mcp.toml` when no specs and no --config are given.
fn resolve_run_inputs(
    specs: Vec<String>,
    config_path: Option<&Path>,
) -> anyhow::Result<(Vec<SpecInput>, Option<CodeMcpConfig>)> {
    if let Some(path) = config_path {
        if !specs.is_empty() {
            anyhow::bail!("cannot use --config with positional spec arguments");
        }
        let config = load_config(path)?;
        let inputs: Vec<SpecInput> = config.apis.iter().map(|(name, entry)| SpecInput {
            name: Some(name.clone()),
            source: entry.spec.clone(),
        }).collect();
        return Ok((inputs, Some(config)));
    }

    if specs.is_empty() {
        // Auto-discover code-mcp.toml in current directory
        let default_path = Path::new("code-mcp.toml");
        if default_path.exists() {
            let config = load_config(default_path)?;
            let inputs: Vec<SpecInput> = config.apis.iter().map(|(name, entry)| SpecInput {
                name: Some(name.clone()),
                source: entry.spec.clone(),
            }).collect();
            return Ok((inputs, Some(config)));
        }
        anyhow::bail!("no specs provided. Pass spec paths/URLs as arguments, use --config, or create code-mcp.toml");
    }

    Ok((specs.iter().map(|s| parse_spec_arg(s)).collect(), None))
}

/// Load a manifest from a directory's manifest.json file.
fn load_manifest(dir: &Path) -> anyhow::Result<Manifest> {
    let manifest_path = dir.join("manifest.json");
    let manifest_str = std::fs::read_to_string(&manifest_path).map_err(|e| {
        anyhow::anyhow!("Failed to read manifest from {}: {}", manifest_path.display(), e)
    })?;
    let manifest: Manifest = serde_json::from_str(&manifest_str)?;
    Ok(manifest)
}

/// Validate MCP-layer auth CLI flags: authority and audience must both be set or both omitted.
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
        _ => anyhow::bail!("--auth-authority and --auth-audience must both be set (or both omitted)"),
    }
}

/// Warn at startup if any API declares auth but no credentials are configured.
fn warn_missing_auth(manifest: &Manifest, auth: &AuthCredentialsMap) {
    for api in &manifest.apis {
        if api.auth.is_some() && !auth.contains_key(&api.name) {
            eprintln!(
                "warning: {}: spec declares auth but no credentials configured. API calls will likely fail with 401.",
                api.name
            );
        }
    }
}

/// Create a `CodeMcpServer` from a manifest and serve it with the given transport.
async fn serve(
    manifest: Manifest,
    transport: &str,
    port: u16,
    mcp_auth: Option<McpAuthConfig>,
    auth: AuthCredentialsMap,
    timeout: u64,
    memory_limit: usize,
    max_api_calls: usize,
) -> anyhow::Result<()> {
    let handler = Arc::new(HttpHandler::new());
    let config = ExecutorConfig {
        timeout_ms: timeout * 1000,
        memory_limit: Some(memory_limit * 1024 * 1024),
        max_api_calls: Some(max_api_calls),
    };
    let server = CodeMcpServer::new(manifest, handler, auth, config);

    match transport {
        "stdio" => serve_stdio(server).await,
        "sse" | "http" => serve_http(server, port, mcp_auth).await,
        other => anyhow::bail!("Unknown transport: '{other}'. Use 'stdio' or 'sse'."),
    }
}

// serve_stdio and serve_http remain exactly the same as current code.
```

Keep `serve_stdio` and `serve_http` unchanged from the current implementation.

**Step 2: Run all tests**

Run: `cargo test`
Expected: PASS

**Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: rewrite main to use --auth/--config instead of auto-derived env vars"
```

---

### Task 6: Remove `load_auth_from_env` and old tests

**Files:**
- Modify: `src/runtime/http.rs:128-158` (delete `load_auth_from_env`)
- Modify: `src/runtime/http.rs:291-337` (delete old env var tests)

**Step 1: Delete `load_auth_from_env`**

Remove the function entirely from `src/runtime/http.rs` (lines 128-158).

**Step 2: Delete the old env var tests**

Remove `test_load_bearer_from_env`, `test_load_api_key_from_env`, `test_load_no_env_returns_empty`, and the `test_manifest_with_api` helper from the test module.

**Step 3: Remove the import in `main.rs`**

The `main.rs` rewrite from Task 5 should already not import `load_auth_from_env`. Verify there are no remaining references.

**Step 4: Run all tests**

Run: `cargo test`
Expected: PASS

**Step 5: Commit**

```bash
git add src/runtime/http.rs
git commit -m "refactor: remove load_auth_from_env and legacy env var convention"
```

---

### Task 7: Update README

**Files:**
- Modify: `README.md`

**Step 1: Update the Quick Start section**

Replace the current Quick Start with:

```markdown
## Quick Start

```bash
cargo install --path .
```

Point at an OpenAPI spec and provide your API key:

```bash
export MY_TOKEN=your-token-here
code-mcp run petstore=https://petstore3.swagger.io/api/v3/openapi.json \
  --auth petstore:MY_TOKEN
```

Or use a config file (`code-mcp.toml`):

```toml
[apis.petstore]
spec = "https://petstore3.swagger.io/api/v3/openapi.json"
auth = "your-token-here"
```

```bash
code-mcp run
```

Add the server to your MCP client config:

```json
{
  "mcpServers": {
    "petstore": {
      "command": "code-mcp",
      "args": ["run", "petstore=https://petstore3.swagger.io/api/v3/openapi.json", "--auth", "petstore:PETSTORE_TOKEN"],
      "env": {
        "PETSTORE_TOKEN": "your-token-here"
      }
    }
  }
}
```
```

**Step 2: Update the Authentication section**

Replace the "Upstream API Credentials" subsection:

```markdown
### Upstream API Credentials

These are the credentials code-mcp uses to call the APIs behind the SDK.

**CLI `--auth` flag** (quick start):

```bash
# Named: --auth name:ENV_VAR
code-mcp run petstore=spec.yaml --auth petstore:MY_TOKEN

# Unnamed (single-spec only): --auth ENV_VAR
code-mcp run spec.yaml --auth MY_TOKEN
```

The tool reads the value of the environment variable at startup. The secret never appears in the command itself.

**Config file** (`code-mcp.toml`):

```toml
[apis.petstore]
spec = "https://petstore.example.com/spec.json"
auth = "sk-my-token"

[apis.stripe]
spec = "./stripe.yaml"
auth = "sk_live_abc123"

[apis.legacy]
spec = "./legacy.yaml"

[apis.legacy.auth]
type = "basic"
username = "admin"
password = "secret"
```

Use `auth_env` instead of `auth` to reference an environment variable:

```toml
[apis.stripe]
spec = "./stripe.yaml"
auth_env = "STRIPE_KEY"
```

Run with a config file:

```bash
code-mcp run --config code-mcp.toml
# Or just have code-mcp.toml in the current directory:
code-mcp run
```

**Per-request via `_meta.auth`** (overrides all, for hosted mode):

```json
{
  "method": "tools/call",
  "params": {
    "name": "execute_script",
    "arguments": { "script": "return sdk.list_pets()" },
    "_meta": {
      "auth": {
        "petstore": { "type": "bearer", "token": "sk-runtime-token" }
      }
    }
  }
}
```

**Resolution order** (first match wins):
1. CLI `--auth` flag
2. Config file `auth` / `auth_env`
3. Per-request `_meta.auth`
```

**Step 3: Update CLI Reference**

Add `--auth` and `--config` to the `code-mcp run` table:

```markdown
| Flag               | Default | Description                                    |
| ------------------ | ------- | ---------------------------------------------- |
| `--config`         | --      | Path to TOML config file                       |
| `--auth`           | --      | API auth: `name:ENV_VAR` or `ENV_VAR`          |
| `--transport`      | `stdio` | Transport type (`stdio`, `sse`)                |
| `--port`           | `8080`  | Port for HTTP/SSE transport                    |
| `--timeout`        | `30`    | Script execution timeout (seconds)             |
| `--memory-limit`   | `64`    | Luau VM memory limit (MB)                      |
| `--max-api-calls`  | `100`   | Max upstream API calls per script              |
| `--auth-authority` | --      | OAuth issuer URL (enables JWT auth)            |
| `--auth-audience`  | --      | Expected JWT audience                          |
| `--auth-jwks-uri`  | --      | Explicit JWKS URI override                     |
```

Add note about auto-discovery: "If no specs and no `--config` are provided, `code-mcp run` looks for `code-mcp.toml` in the current directory."

**Step 4: Commit**

```bash
git add README.md
git commit -m "docs: update README for new auth UX with --auth flag and config file"
```

---

### Task 8: Final integration test

**Files:**
- Modify: `tests/full_roundtrip.rs` (add a config-file-based test)

**Step 1: Add integration test for config-based workflow**

Add to `tests/full_roundtrip.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_roundtrip_with_named_spec() {
    use code_mcp::config::SpecInput;

    let output_dir = tempfile::tempdir().unwrap();
    generate(
        &[SpecInput {
            name: Some("mystore".to_string()),
            source: "testdata/petstore.yaml".to_string(),
        }],
        output_dir.path(),
    )
    .await
    .unwrap();

    let manifest_str = std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap();
    let manifest: Manifest = serde_json::from_str(&manifest_str).unwrap();

    // API name should be the user-chosen name
    assert_eq!(manifest.apis[0].name, "mystore");

    // Functions should reference the user-chosen name
    for func in &manifest.functions {
        assert_eq!(func.api, "mystore");
    }

    // Execute a script to verify the SDK still works with the custom name
    let handler = HttpHandler::mock(|method, url, _query, _body| {
        if method == "GET" && url.contains("/pets/") {
            Ok(serde_json::json!({"id": "pet-1", "name": "Buddy", "status": "available"}))
        } else {
            Err(anyhow::anyhow!("unexpected: {} {}", method, url))
        }
    });

    let executor = ScriptExecutor::new(manifest, Arc::new(handler), ExecutorConfig::default());
    let auth = AuthCredentialsMap::new();

    let result = executor
        .execute(
            "local pet = sdk.get_pet_by_id('pet-1')\nreturn pet.name",
            &auth,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.result, serde_json::json!("Buddy"));
}
```

**Step 2: Run all tests**

Run: `cargo test`
Expected: all PASS

**Step 3: Commit**

```bash
git add tests/full_roundtrip.rs
git commit -m "test: add integration test for named spec workflow"
```
