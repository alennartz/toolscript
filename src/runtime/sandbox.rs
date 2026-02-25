use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mlua::{FromLua, Lua, MultiValue, Value};

/// Configuration for the Lua sandbox.
#[derive(Clone, Copy)]
pub struct SandboxConfig {
    /// Maximum memory the Lua VM may allocate (in bytes). Default: 64 MB.
    pub memory_limit: Option<usize>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            memory_limit: Some(64 * 1024 * 1024),
        }
    }
}

/// A locked-down Luau environment using native sandbox mode.
pub struct Sandbox {
    lua: Lua,
    logs: Arc<Mutex<Vec<String>>>,
}

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

impl Sandbox {
    /// Create a new sandboxed Luau environment.
    ///
    /// Uses Luau's native sandbox mode which makes all globals and metatables
    /// read-only, creates isolated per-script environments, and restricts
    /// `collectgarbage`. Custom `print()`, `json`, and `sdk` globals are
    /// injected before sandboxing activates.
    pub fn new(config: SandboxConfig) -> anyhow::Result<Self> {
        let lua = Lua::new();

        // Set memory limit before sandboxing
        if let Some(limit) = config.memory_limit {
            lua.set_memory_limit(limit)?;
        }

        // Shared log buffer for captured print output
        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        // Override print() to capture output
        let logs_clone = Arc::clone(&logs);
        let print_fn = lua.create_function(move |_, args: MultiValue| {
            let parts: Vec<String> = args.iter().map(format_lua_value).collect();
            let line = parts.join("\t");
            if let Ok(mut logs) = logs_clone.lock() {
                logs.push(line);
            }
            Ok(())
        })?;
        lua.globals().set("print", print_fn)?;

        // Add json.encode() and json.decode() — Rust-backed via serde
        let json_table = lua.create_table()?;

        let encode_fn = lua.create_function(|lua, value: Value| {
            use mlua::LuaSerdeExt;
            let json_value: serde_json::Value = lua.from_value(value)?;
            serde_json::to_string(&json_value).map_err(mlua::Error::external)
        })?;
        json_table.set("encode", encode_fn)?;

        let decode_fn = lua.create_function(|lua, s: String| {
            use mlua::LuaSerdeExt;
            let json_value: serde_json::Value =
                serde_json::from_str(&s).map_err(mlua::Error::external)?;
            lua.to_value(&json_value)
        })?;
        json_table.set("decode", decode_fn)?;

        lua.globals().set("json", json_table)?;

        // Create empty sdk table (will be populated by registry)
        let sdk_table = lua.create_table()?;
        lua.globals().set("sdk", sdk_table)?;

        // Remove `require` — Luau sandbox mode does not strip it, and we
        // do not want scripts loading external modules.
        lua.globals().set("require", Value::Nil)?;

        Ok(Self { lua, logs })
    }

    /// Evaluate a Lua script and return the result.
    pub fn eval<T: FromLua>(&self, script: &str) -> anyhow::Result<T> {
        let result = self.lua.load(script).eval::<T>()?;
        Ok(result)
    }

    /// Evaluate a Lua script and return both the result and captured logs.
    pub fn eval_with_logs<T: FromLua>(&self, script: &str) -> anyhow::Result<(T, Vec<String>)> {
        let result = self.lua.load(script).eval::<T>()?;
        let logs = self.take_logs();
        Ok((result, logs))
    }

    /// Enable Luau sandbox mode.
    ///
    /// This makes all globals (including custom ones like `sdk`, `json`, and
    /// `print`) read-only and activates per-script isolated environments.
    /// Must be called **after** all globals have been set up (e.g. after
    /// registry functions are registered into the `sdk` table).
    pub fn enable_sandbox(&self) -> anyhow::Result<()> {
        self.lua.sandbox(true)?;
        Ok(())
    }

    /// Register the `file` table with a `save(filename, content)` function.
    ///
    /// Must be called BEFORE `enable_sandbox()`. Returns a shared tracker
    /// that records all files written during this sandbox's lifetime.
    pub fn register_file_save(
        &self,
        output_dir: PathBuf,
        max_bytes: u64,
    ) -> anyhow::Result<Arc<Mutex<Vec<FileWritten>>>> {
        use std::sync::atomic::{AtomicU64, Ordering};

        let files_written: Arc<Mutex<Vec<FileWritten>>> = Arc::new(Mutex::new(Vec::new()));
        let bytes_written = Arc::new(AtomicU64::new(0));

        let files_clone = Arc::clone(&files_written);
        let bytes_clone = Arc::clone(&bytes_written);

        let save_fn =
            self.lua
                .create_function(move |_, (filename, content): (String, String)| {
                    // Validate filename
                    if filename.is_empty() {
                        return Err(mlua::Error::external(anyhow::anyhow!(
                            "filename cannot be empty"
                        )));
                    }
                    if filename.contains('\0') {
                        return Err(mlua::Error::external(anyhow::anyhow!(
                            "filename cannot contain null bytes"
                        )));
                    }
                    let path = std::path::Path::new(&filename);
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

    /// Access the raw Lua state (for registry to add functions).
    pub const fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Drain and return all captured log lines.
    pub fn take_logs(&self) -> Vec<String> {
        let Ok(mut logs) = self.logs.lock() else {
            return Vec::new();
        };
        std::mem::take(&mut *logs)
    }
}

/// Format a Lua value for print output.
fn format_lua_value(value: &Value) -> String {
    match value {
        Value::Nil => "nil".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Number(n) =>
        {
            #[allow(
                clippy::float_cmp,
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation
            )]
            if *n == (*n as i64) as f64 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        Value::String(s) => s.to_string_lossy(),
        Value::Table(_) => "table".to_string(),
        Value::Function(_) => "function".to_string(),
        Value::UserData(_) | Value::LightUserData(_) => "userdata".to_string(),
        Value::Thread(_) => "thread".to_string(),
        Value::Error(e) => format!("error: {e}"),
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Helper: create a sandbox with sandboxing enabled.
    fn sandboxed() -> Sandbox {
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        sb.enable_sandbox().unwrap();
        sb
    }

    #[test]
    fn test_sandbox_allows_basic_lua() {
        let sb = sandboxed();
        let result: String = sb.eval("return 'hello'").unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_sandbox_allows_string_lib() {
        let sb = sandboxed();
        let result: String = sb.eval("return string.upper('hello')").unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn test_sandbox_allows_table_lib() {
        let sb = sandboxed();
        let result: String = sb
            .eval(
                r#"
                local t = {3, 1, 2}
                table.sort(t)
                return table.concat(t, ",")
            "#,
            )
            .unwrap();
        assert_eq!(result, "1,2,3");
    }

    #[test]
    fn test_sandbox_allows_math_lib() {
        let sb = sandboxed();
        let result: f64 = sb.eval("return math.floor(3.7)").unwrap();
        assert!((result - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_sandbox_blocks_io() {
        let sb = sandboxed();
        let result = sb.eval::<Value>("return io.open('/etc/passwd')");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_blocks_os_execute() {
        let sb = sandboxed();
        // Luau's os table only has clock/difftime/time — execute doesn't exist
        let result = sb.eval::<Value>("return os.execute('ls')");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_blocks_loadfile() {
        let sb = sandboxed();
        let result = sb.eval::<Value>("return loadfile('test.lua')");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_blocks_require() {
        let sb = sandboxed();
        let result = sb.eval::<Value>("return require('os')");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_blocks_dofile() {
        let sb = sandboxed();
        let result = sb.eval::<Value>("return dofile('test.lua')");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_blocks_string_dump() {
        let sb = sandboxed();
        let result = sb.eval::<Value>("return string.dump(function() end)");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_captures_print() {
        let sb = sandboxed();
        let (_, logs) = sb
            .eval_with_logs::<Value>(
                r#"
                print("hello")
                print("world")
            "#,
            )
            .unwrap();
        assert_eq!(logs, vec!["hello", "world"]);
    }

    #[test]
    fn test_sandbox_json_encode_decode() {
        let sb = sandboxed();
        let result: String = sb
            .eval(
                r#"
                local t = {name = "test", value = 42}
                local encoded = json.encode(t)
                local decoded = json.decode(encoded)
                return decoded.name
            "#,
            )
            .unwrap();
        assert_eq!(result, "test");
    }

    #[test]
    fn test_sandbox_has_sdk_table() {
        let sb = sandboxed();
        let result: String = sb.eval("return type(sdk)").unwrap();
        assert_eq!(result, "table");
    }

    #[test]
    fn test_file_save_basic() {
        let dir = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024)
            .unwrap();
        sb.enable_sandbox().unwrap();

        let result: String = sb
            .eval(r#"return file.save("test.txt", "hello world")"#)
            .unwrap();

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
        sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024)
            .unwrap();
        sb.enable_sandbox().unwrap();

        sb.eval::<String>(r#"return file.save("data/results.csv", "a,b,c")"#)
            .unwrap();

        let content = std::fs::read_to_string(dir.path().join("data/results.csv")).unwrap();
        assert_eq!(content, "a,b,c");
    }

    #[test]
    fn test_file_save_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024)
            .unwrap();
        sb.enable_sandbox().unwrap();

        let result = sb.eval::<Value>(r#"return file.save("../evil.txt", "pwned")"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_file_save_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024)
            .unwrap();
        sb.enable_sandbox().unwrap();

        let result = sb.eval::<Value>(r#"return file.save("/etc/passwd", "pwned")"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_file_save_rejects_null_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        sb.register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024)
            .unwrap();
        sb.enable_sandbox().unwrap();

        // Use Lua string escape for null byte
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
        sb.eval::<String>(r#"return file.save("a.txt", "hello")"#)
            .unwrap();
        // Second write: 6 bytes, should fail (total would be 11 > 10)
        let result = sb.eval::<Value>(r#"return file.save("b.txt", "world!")"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_file_save_tracks_files_written() {
        let dir = tempfile::tempdir().unwrap();
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let tracker = sb
            .register_file_save(dir.path().to_path_buf(), 50 * 1024 * 1024)
            .unwrap();
        sb.enable_sandbox().unwrap();

        sb.eval::<String>(r#"return file.save("a.txt", "aaa")"#)
            .unwrap();
        sb.eval::<String>(r#"return file.save("b.txt", "bbb")"#)
            .unwrap();

        let files = tracker.lock().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "a.txt");
        assert_eq!(files[0].bytes, 3);
        assert_eq!(files[1].name, "b.txt");
        assert_eq!(files[1].bytes, 3);
        drop(files);
    }

    #[test]
    fn test_sandbox_memory_limit() {
        let sb = Sandbox::new(SandboxConfig {
            memory_limit: Some(1024 * 1024), // 1 MB
        })
        .unwrap();
        sb.enable_sandbox().unwrap();
        let result = sb.eval::<Value>(
            r#"
            local s = "x"
            for i = 1, 30 do
                s = s .. s
            end
            return s
        "#,
        );
        assert!(result.is_err());
    }
}
