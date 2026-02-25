# `file.save()` Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `file.save(filename, content)` Luau global that lets scripts safely persist data to a configured output directory.

**Architecture:** Register `file.save()` as a sandbox global (like `json` and `print`) backed by a Rust closure that validates filenames, enforces size limits, and writes to a single configured output directory. Track files written per execution and include them in the MCP response. Disable in hosted mode.

**Tech Stack:** Rust, mlua (Luau bindings), TOML config (serde), existing test infrastructure (cargo test + Python e2e)

---

### Task 1: Add `OutputConfig` to config

**Files:**
- Modify: `src/config.rs`

**Step 1: Write the failing test**

Add to the `tests` module in `src/config.rs`:

```rust
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
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_load_config_with_output -- --nocapture`
Expected: FAIL — `output` field does not exist on `CodeMcpConfig`

**Step 3: Write minimal implementation**

Add the struct and field to `src/config.rs`:

```rust
/// Output configuration for file.save() in scripts.
#[derive(Debug, Clone, Deserialize)]
pub struct OutputConfig {
    pub dir: Option<String>,
    pub max_bytes: Option<u64>,
    pub enabled: Option<bool>,
}
```

Add the field to `CodeMcpConfig`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct CodeMcpConfig {
    pub apis: HashMap<String, ConfigApiEntry>,
    #[serde(default)]
    pub frozen_params: Option<HashMap<String, String>>,
    #[serde(default)]
    pub output: Option<OutputConfig>,
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test test_load_config_with_output test_load_config_without_output -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: add OutputConfig to TOML config parsing"
```

---

### Task 2: Add `--output-dir` CLI flag

**Files:**
- Modify: `src/cli.rs`

**Step 1: Write the failing test**

Add to `tests` module in `src/cli.rs`:

```rust
#[test]
fn test_run_with_output_dir() {
    let cli = Cli::parse_from(["code-mcp", "run", "spec.yaml", "--output-dir", "/tmp/out"]);
    match cli.command {
        Command::Run { output_dir, .. } => {
            assert_eq!(output_dir.as_deref(), Some("/tmp/out"));
        }
        _ => panic!("expected Run"),
    }
}

#[test]
fn test_serve_with_output_dir() {
    let cli = Cli::parse_from(["code-mcp", "serve", "./output", "--output-dir", "/tmp/out"]);
    match cli.command {
        Command::Serve { output_dir, .. } => {
            assert_eq!(output_dir.as_deref(), Some("/tmp/out"));
        }
        _ => panic!("expected Serve"),
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_run_with_output_dir -- --nocapture`
Expected: FAIL — `output_dir` field does not exist

**Step 3: Write minimal implementation**

Add to both `Serve` and `Run` variants in `src/cli.rs`:

```rust
/// Output directory for file.save() in scripts
#[arg(long)]
output_dir: Option<String>,
```

**Step 4: Run tests to verify they pass**

Run: `cargo test test_run_with_output_dir test_serve_with_output_dir -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/cli.rs
git commit -m "feat: add --output-dir CLI flag to serve and run commands"
```

---

### Task 3: Add `file.save()` to sandbox

This is the core task. The `Sandbox` gets a `register_file_save()` method that injects the `file` table with a `save` function.

**Files:**
- Modify: `src/runtime/sandbox.rs`

**Step 1: Write the failing tests**

Add to `tests` module in `src/runtime/sandbox.rs`:

```rust
use std::path::PathBuf;

#[test]
fn test_file_save_basic() {
    let dir = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024).unwrap();
    sb.enable_sandbox().unwrap();

    let result: String = sb.eval(&format!(
        r#"return file.save("test.txt", "hello world")"#
    )).unwrap();

    // Should return absolute path
    assert!(result.contains("test.txt"));
    // File should exist on disk
    let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "hello world");
}

#[test]
fn test_file_save_subdirectory() {
    let dir = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024).unwrap();
    sb.enable_sandbox().unwrap();

    sb.eval::<String>(r#"return file.save("data/results.csv", "a,b,c")"#).unwrap();

    let content = std::fs::read_to_string(dir.path().join("data/results.csv")).unwrap();
    assert_eq!(content, "a,b,c");
}

#[test]
fn test_file_save_rejects_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024).unwrap();
    sb.enable_sandbox().unwrap();

    let result = sb.eval::<Value>(r#"return file.save("../evil.txt", "pwned")"#);
    assert!(result.is_err());
}

#[test]
fn test_file_save_rejects_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024).unwrap();
    sb.enable_sandbox().unwrap();

    let result = sb.eval::<Value>(r#"return file.save("/etc/passwd", "pwned")"#);
    assert!(result.is_err());
}

#[test]
fn test_file_save_rejects_null_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024).unwrap();
    sb.enable_sandbox().unwrap();

    let result = sb.eval::<Value>(r#"return file.save("te\0st.txt", "data")"#);
    assert!(result.is_err());
}

#[test]
fn test_file_save_enforces_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    sb.register_file_save(dir.path().to_path_buf(), 10).unwrap(); // 10 byte limit
    sb.enable_sandbox().unwrap();

    // First write: 5 bytes, should succeed
    sb.eval::<String>(r#"return file.save("a.txt", "hello")"#).unwrap();
    // Second write: 6 bytes, should fail (total would be 11 > 10)
    let result = sb.eval::<Value>(r#"return file.save("b.txt", "world!")"#);
    assert!(result.is_err());
}

#[test]
fn test_file_save_tracks_files_written() {
    let dir = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    let tracker = sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024).unwrap();
    sb.enable_sandbox().unwrap();

    sb.eval::<String>(r#"return file.save("a.txt", "aaa")"#).unwrap();
    sb.eval::<String>(r#"return file.save("b.txt", "bbb")"#).unwrap();

    let files = tracker.lock().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].name, "a.txt");
    assert_eq!(files[0].bytes, 3);
    assert_eq!(files[1].name, "b.txt");
    assert_eq!(files[1].bytes, 3);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test test_file_save -- --nocapture`
Expected: FAIL — `register_file_save` does not exist

**Step 3: Write implementation**

Add to `src/runtime/sandbox.rs`, above the `impl Sandbox` block:

```rust
/// Record of a file written by a script via `file.save()`.
#[derive(Debug, Clone)]
pub struct FileWritten {
    /// The relative filename as provided by the script.
    pub name: String,
    /// The resolved absolute path on disk.
    pub path: String,
    /// Number of bytes written.
    pub bytes: u64,
}
```

Add method to `impl Sandbox`:

```rust
/// Register the `file` table with a `save(filename, content)` function.
///
/// Must be called BEFORE `enable_sandbox()`. Returns a shared tracker
/// that records all files written during this sandbox's lifetime.
pub fn register_file_save(
    &self,
    output_dir: PathBuf,
    max_bytes: u64,
) -> anyhow::Result<Arc<Mutex<Vec<FileWritten>>>> {
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    let files_written: Arc<Mutex<Vec<FileWritten>>> = Arc::new(Mutex::new(Vec::new()));
    let bytes_written = Arc::new(AtomicU64::new(0));

    let files_clone = Arc::clone(&files_written);
    let bytes_clone = Arc::clone(&bytes_written);

    let save_fn = self.lua.create_function(move |_, (filename, content): (String, String)| {
        // Validate filename
        if filename.is_empty() {
            return Err(mlua::Error::external(anyhow::anyhow!("filename cannot be empty")));
        }
        if filename.contains('\0') {
            return Err(mlua::Error::external(anyhow::anyhow!("filename cannot contain null bytes")));
        }
        let path = Path::new(&filename);
        if path.is_absolute() {
            return Err(mlua::Error::external(anyhow::anyhow!(
                "filename must be relative, got absolute path"
            )));
        }
        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(mlua::Error::external(anyhow::anyhow!(
                    "filename cannot contain '..' path traversal"
                )));
            }
        }

        // Check size limit
        let content_bytes = content.len() as u64;
        let prev = bytes_clone.fetch_add(content_bytes, Ordering::SeqCst);
        if prev + content_bytes > max_bytes {
            bytes_clone.fetch_sub(content_bytes, Ordering::SeqCst);
            return Err(mlua::Error::external(anyhow::anyhow!(
                "output size limit exceeded ({max_bytes} bytes)"
            )));
        }

        // Write file
        let full_path = output_dir.join(&filename);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).map_err(mlua::Error::external)?;
        }
        std::fs::write(&full_path, &content).map_err(mlua::Error::external)?;

        // Track
        let abs_path = full_path.to_string_lossy().to_string();
        if let Ok(mut files) = files_clone.lock() {
            files.push(FileWritten {
                name: filename,
                path: abs_path.clone(),
                bytes: content_bytes,
            });
        }

        Ok(abs_path)
    })?;

    let file_table = self.lua.create_table()?;
    file_table.set("save", save_fn)?;
    self.lua.globals().set("file", file_table)?;

    Ok(files_written)
}
```

Add required imports at the top of the file:

```rust
use std::path::PathBuf;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test test_file_save -- --nocapture`
Expected: All 7 tests PASS

**Step 5: Commit**

```bash
git add src/runtime/sandbox.rs
git commit -m "feat: add file.save() sandbox global with validation and size limits"
```

---

### Task 4: Wire output through executor

**Files:**
- Modify: `src/runtime/executor.rs`

**Step 1: Write the failing test**

Add to the `tests` module in `src/runtime/executor.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_execute_file_save() {
    let output_dir = tempfile::tempdir().unwrap();
    let executor = ScriptExecutor::new(
        empty_manifest(),
        Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
        ExecutorConfig::default(),
        Some(OutputConfig {
            dir: output_dir.path().to_path_buf(),
            max_bytes: 50 * 1024 * 1024,
        }),
    );
    let auth = AuthCredentialsMap::new();

    let result = executor
        .execute(
            r#"
            file.save("test.json", '{"hello":"world"}')
            return "done"
        "#,
            &auth,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.result, serde_json::json!("done"));
    assert_eq!(result.files_written.len(), 1);
    assert_eq!(result.files_written[0].name, "test.json");

    // Verify file exists on disk
    let content = std::fs::read_to_string(output_dir.path().join("test.json")).unwrap();
    assert_eq!(content, r#"{"hello":"world"}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_execute_no_file_save_when_disabled() {
    let executor = ScriptExecutor::new(
        empty_manifest(),
        Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
        ExecutorConfig::default(),
        None, // output disabled
    );
    let auth = AuthCredentialsMap::new();

    // file table should not exist, so this should error
    let result = executor
        .execute(r#"return file.save("test.txt", "data")"#, &auth, None)
        .await;

    assert!(result.is_err());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test test_execute_file_save -- --nocapture`
Expected: FAIL — constructor signature mismatch

**Step 3: Write implementation**

Add `OutputConfig` struct to `src/runtime/executor.rs` (a resolved config, not optional fields):

```rust
/// Resolved output configuration for file.save().
pub struct OutputConfig {
    /// Directory where files will be written.
    pub dir: PathBuf,
    /// Maximum total bytes that can be written per script execution.
    pub max_bytes: u64,
}
```

Add `files_written` to `ExecutionResult`:

```rust
use crate::runtime::sandbox::FileWritten;

pub struct ExecutionResult {
    pub result: serde_json::Value,
    pub logs: Vec<String>,
    pub stats: ExecutionStats,
    pub files_written: Vec<FileWritten>,
}
```

Add `output_config` to `ScriptExecutor`:

```rust
pub struct ScriptExecutor {
    manifest: Manifest,
    handler: Arc<HttpHandler>,
    config: ExecutorConfig,
    output_config: Option<OutputConfig>,
}
```

Update constructor:

```rust
pub const fn new(
    manifest: Manifest,
    handler: Arc<HttpHandler>,
    config: ExecutorConfig,
    output_config: Option<OutputConfig>,
) -> Self {
    Self {
        manifest,
        handler,
        config,
        output_config,
    }
}
```

Update `execute()` — after registry registration and before `enable_sandbox()`:

```rust
// 3c. Register file.save() if output is configured
let files_tracker = if let Some(ref output_config) = self.output_config {
    Some(sandbox.register_file_save(
        output_config.dir.clone(),
        output_config.max_bytes,
    )?)
} else {
    None
};
```

After execution, collect files_written:

```rust
let files_written = files_tracker
    .and_then(|t| t.lock().ok().map(|g| g.clone()))
    .unwrap_or_default();
```

Include in result:

```rust
Ok(ExecutionResult {
    result: result_json,
    logs,
    stats: ExecutionStats {
        api_calls,
        duration_ms,
    },
    files_written,
})
```

Update all existing `ScriptExecutor::new()` call sites to pass `None` for `output_config`:
- `src/runtime/executor.rs` tests
- `tests/full_roundtrip.rs`
- `src/server/mod.rs`

**Step 4: Run tests to verify they pass**

Run: `cargo test -- --nocapture`
Expected: All tests PASS (existing + new)

**Step 5: Commit**

```bash
git add src/runtime/executor.rs src/runtime/sandbox.rs tests/full_roundtrip.rs src/server/mod.rs
git commit -m "feat: wire file.save() through executor with OutputConfig"
```

---

### Task 5: Include `files_written` in MCP response

**Files:**
- Modify: `src/server/tools.rs`

**Step 1: Write the failing test**

Add to `tests` module in `src/server/mod.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_execute_script_includes_files_written() {
    let output_dir = tempfile::tempdir().unwrap();
    let server = CodeMcpServer::new(
        test_manifest(),
        Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
        AuthCredentialsMap::new(),
        ExecutorConfig::default(),
        Some(crate::runtime::executor::OutputConfig {
            dir: output_dir.path().to_path_buf(),
            max_bytes: 50 * 1024 * 1024,
        }),
    );

    let merged_auth = AuthCredentialsMap::new();
    let result = server
        .executor
        .execute(
            r#"file.save("test.txt", "hello"); return "ok""#,
            &merged_auth,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.files_written.len(), 1);
    assert_eq!(result.files_written[0].name, "test.txt");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_execute_script_includes_files_written -- --nocapture`
Expected: FAIL — `CodeMcpServer::new` signature mismatch

**Step 3: Write implementation**

Update `CodeMcpServer::new()` in `src/server/mod.rs` to accept `output_config`:

```rust
pub fn new(
    manifest: Manifest,
    handler: Arc<HttpHandler>,
    auth: AuthCredentialsMap,
    config: ExecutorConfig,
    output_config: Option<OutputConfig>,
) -> Self {
    // ...
    let executor = ScriptExecutor::new(manifest.clone(), handler, config, output_config);
    // ...
}
```

Update `execute_script_async` in `src/server/tools.rs` to include `files_written`:

```rust
let response = serde_json::json!({
    "result": exec_result.result,
    "logs": exec_result.logs,
    "stats": {
        "api_calls": exec_result.stats.api_calls,
        "duration_ms": exec_result.stats.duration_ms,
    },
    "files_written": exec_result.files_written.iter().map(|f| {
        serde_json::json!({
            "name": f.name,
            "path": f.path,
            "bytes": f.bytes,
        })
    }).collect::<Vec<_>>(),
});
```

Update all `CodeMcpServer::new()` call sites to pass `None`:
- `src/server/mod.rs` test helper `test_server()`
- `src/main.rs` — will be properly wired in Task 6

**Step 4: Run tests to verify they pass**

Run: `cargo test -- --nocapture`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add src/server/mod.rs src/server/tools.rs src/main.rs
git commit -m "feat: include files_written in execute_script MCP response"
```

---

### Task 6: Wire config through main.rs

**Files:**
- Modify: `src/main.rs`

**Step 1: Write implementation**

This task is wiring only — no new tests needed (covered by integration/e2e tests).

Add a helper function to `src/main.rs`:

```rust
use code_mcp::runtime::executor::OutputConfig;

/// Build the resolved output config from CLI flags, TOML config, and mode.
fn resolve_output_config(
    cli_output_dir: Option<&str>,
    config: Option<&CodeMcpConfig>,
    is_hosted: bool,
) -> Option<OutputConfig> {
    // If hosted mode and no explicit CLI override, disable
    if is_hosted && cli_output_dir.is_none() {
        if let Some(cfg) = config {
            if let Some(ref output) = cfg.output {
                if output.enabled == Some(true) {
                    // Explicitly enabled in config — honor it even in hosted mode
                } else {
                    return None;
                }
            } else {
                return None;
            }
        } else {
            return None;
        }
    }

    let dir = cli_output_dir
        .map(PathBuf::from)
        .or_else(|| {
            config
                .and_then(|c| c.output.as_ref())
                .and_then(|o| o.dir.as_ref())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("./code-mcp-output"));

    let max_bytes = config
        .and_then(|c| c.output.as_ref())
        .and_then(|o| o.max_bytes)
        .unwrap_or(50 * 1024 * 1024);

    // Check if explicitly disabled in config
    if let Some(cfg) = config {
        if let Some(ref output) = cfg.output {
            if output.enabled == Some(false) && cli_output_dir.is_none() {
                return None;
            }
        }
    }

    Some(OutputConfig { dir, max_bytes })
}
```

Update `ServeArgs`:

```rust
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
}
```

Update `serve()`:

```rust
let server = CodeMcpServer::new(args.manifest, handler, args.auth, config, args.output_config);
```

Update both `Serve` and `Run` command handlers in `main()` to resolve and pass output config:

```rust
// In Serve command handler:
let output_config = resolve_output_config(
    output_dir.as_deref(),
    None, // no TOML config for bare serve
    mcp_auth.is_some(),
);

// In Run command handler:
let output_config = resolve_output_config(
    output_dir.as_deref(),
    config_obj.as_ref(),
    mcp_auth.is_some(),
);
```

**Step 2: Run all tests**

Run: `cargo test -- --nocapture`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire output config from CLI and TOML through to server"
```

---

### Task 7: Integration test

**Files:**
- Modify: `tests/full_roundtrip.rs`

**Step 1: Write the test**

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_file_save_roundtrip() {
    let output_dir = tempfile::tempdir().unwrap();
    let spec_output = tempfile::tempdir().unwrap();
    let no_frozen: HashMap<String, HashMap<String, String>> = HashMap::new();

    generate(
        &[SpecInput {
            name: None,
            source: "testdata/petstore.yaml".to_string(),
        }],
        spec_output.path(),
        &HashMap::new(),
        &no_frozen,
    )
    .await
    .unwrap();

    let manifest_str =
        std::fs::read_to_string(spec_output.path().join("manifest.json")).unwrap();
    let manifest: Manifest = serde_json::from_str(&manifest_str).unwrap();

    let handler = HttpHandler::mock(|method, url, _query, _body| {
        if method == "GET" && url.ends_with("/pets") {
            Ok(serde_json::json!([
                {"id": "1", "name": "Buddy", "status": "available"},
                {"id": "2", "name": "Max", "status": "pending"}
            ]))
        } else {
            Err(anyhow::anyhow!("unexpected: {method} {url}"))
        }
    });

    let executor = ScriptExecutor::new(
        manifest,
        Arc::new(handler),
        ExecutorConfig::default(),
        Some(code_mcp::runtime::executor::OutputConfig {
            dir: output_dir.path().to_path_buf(),
            max_bytes: 50 * 1024 * 1024,
        }),
    );
    let auth = AuthCredentialsMap::new();

    let result = executor
        .execute(
            r#"
            local pets = sdk.list_pets()
            local csv = "id,name,status\n"
            for _, p in ipairs(pets) do
                csv = csv .. p.id .. "," .. p.name .. "," .. p.status .. "\n"
            end
            file.save("pets.csv", csv)
            file.save("summary.json", json.encode({ count = #pets }))
            return "saved"
        "#,
            &auth,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.result, serde_json::json!("saved"));
    assert_eq!(result.files_written.len(), 2);

    // Verify CSV file
    let csv = std::fs::read_to_string(output_dir.path().join("pets.csv")).unwrap();
    assert!(csv.contains("Buddy"));
    assert!(csv.contains("Max"));

    // Verify JSON file
    let json_str = std::fs::read_to_string(output_dir.path().join("summary.json")).unwrap();
    let summary: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(summary["count"], 2);
}
```

**Step 2: Run the test**

Run: `cargo test test_file_save_roundtrip -- --nocapture`
Expected: PASS

**Step 3: Commit**

```bash
git add tests/full_roundtrip.rs
git commit -m "test: add file.save() integration roundtrip test"
```

---

### Task 8: E2e test

**Files:**
- Modify: `e2e/tests/test_stdio_scripts.py`
- Modify: `e2e/tests/conftest.py`

**Step 1: Update conftest to add a session with output enabled**

Add to `e2e/tests/conftest.py` — a new fixture that launches code-mcp with `--output-dir`:

```python
@pytest_asyncio.fixture(loop_scope="session", scope="session")
async def mcp_output_session(code_mcp_binary: Path, openapi_spec_url: str, tmp_path_factory):
    """code-mcp instance with file.save() output enabled."""
    output_dir = tmp_path_factory.mktemp("code-mcp-output")
    env = {
        "PATH": "/usr/bin:/bin",
        "TEST_API_BEARER_TOKEN": "test-secret-123",
    }
    server_params = StdioServerParameters(
        command=str(code_mcp_binary),
        args=[
            "run", openapi_spec_url,
            "--auth", "TEST_API_BEARER_TOKEN",
            "--output-dir", str(output_dir),
        ],
        env=env,
    )
    session_ready = asyncio.get_event_loop().create_future()
    shutdown_event = asyncio.Event()

    async def _run():
        try:
            async with stdio_client(server_params) as (read, write):
                async with ClientSession(read, write) as session:
                    await session.initialize()
                    session_ready.set_result((session, output_dir))
                    await shutdown_event.wait()
        except Exception as exc:
            if not session_ready.done():
                session_ready.set_exception(exc)

    task = asyncio.create_task(_run())
    session, out_dir = await session_ready
    yield session, out_dir
    shutdown_event.set()
    try:
        await asyncio.wait_for(task, timeout=5.0)
    except (asyncio.TimeoutError, Exception):
        task.cancel()
        try:
            await task
        except (asyncio.CancelledError, Exception):
            pass
```

**Step 2: Write e2e test**

Add to `e2e/tests/test_stdio_scripts.py`:

```python
@pytest.mark.asyncio
async def test_file_save_writes_to_disk(mcp_output_session):
    """file.save() should write a file and report it in files_written."""
    session, output_dir = mcp_output_session
    result = await session.call_tool("execute_script", {
        "script": '''
            local pets = sdk.list_pets()
            local csv = "id,name\\n"
            for _, p in ipairs(pets.items) do
                csv = csv .. p.id .. "," .. p.name .. "\\n"
            end
            file.save("pets.csv", csv)
            return { saved = true, count = #pets.items }
        '''
    })
    data = parse_result(result)
    assert data["result"]["saved"] is True
    assert data["result"]["count"] == 4

    # Check files_written in response
    assert "files_written" in data
    assert len(data["files_written"]) == 1
    assert data["files_written"][0]["name"] == "pets.csv"
    assert data["files_written"][0]["bytes"] > 0

    # Verify file on disk
    csv_path = output_dir / "pets.csv"
    assert csv_path.exists()
    content = csv_path.read_text()
    assert "Fido" in content
    assert "Whiskers" in content


@pytest.mark.asyncio
async def test_file_save_rejects_traversal(mcp_output_session):
    """file.save() should reject path traversal attempts."""
    session, _ = mcp_output_session
    result = await session.call_tool("execute_script", {
        "script": 'return file.save("../evil.txt", "pwned")'
    })
    assert result.isError is True
    text = result.content[0].text
    assert "traversal" in text.lower() or "error" in text.lower()
```

**Step 3: Build and run e2e tests**

Run:
```bash
cargo build --release
cd e2e && python -m pytest tests/test_stdio_scripts.py::test_file_save_writes_to_disk tests/test_stdio_scripts.py::test_file_save_rejects_traversal -v
```
Expected: PASS

**Step 4: Commit**

```bash
git add e2e/tests/conftest.py e2e/tests/test_stdio_scripts.py
git commit -m "test: add file.save() e2e tests"
```
