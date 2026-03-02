# Sandboxed Luau `io` Library Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace `file.save()` with a Rust-backed standard Lua `io` library sandboxed to a configurable directory, plus `os.remove()` and `io.list()`.

**Architecture:** Implement `io.open()`, `io.lines()`, `io.type()`, `io.list()` and `os.remove()` as Rust-backed Luau globals using mlua UserData for file handles. All path operations are validated (relative only, no `..`, no null bytes) and confined to a single directory. The existing `register_file_save()` method is replaced by `register_io()`. Config migrates from `[output]` to `[io]`, CLI from `--output-dir` to `--io-dir`.

**Tech Stack:** Rust, mlua (Luau bindings), serde/toml for config, tokio for async executor

---

### Task 1: Rename config section from `[output]` to `[io]`

**Files:**
- Modify: `src/config.rs:42-48` (rename `OutputConfig` → `IoConfig`, field on `ToolScriptConfig`)
- Modify: `src/config.rs:62-71` (`ToolScriptConfig.output` → `ToolScriptConfig.io`)
- Modify: `src/config.rs:676-708` (update tests)

**Step 1: Write failing test**

In `src/config.rs` tests, replace `test_load_config_with_output` with:

```rust
#[test]
fn test_load_config_with_io() {
    let toml_content = r#"
[io]
dir = "/tmp/my-files"
max_bytes = 1048576
enabled = false

[apis.petstore]
spec = "petstore.yaml"
"#;
    let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
    tmpfile.write_all(toml_content.as_bytes()).unwrap();

    let config = load_config(tmpfile.path()).unwrap();
    let io_cfg = config.io.unwrap();
    assert_eq!(io_cfg.dir.as_deref(), Some("/tmp/my-files"));
    assert_eq!(io_cfg.max_bytes, Some(1_048_576));
    assert_eq!(io_cfg.enabled, Some(false));
}
```

And rename `test_load_config_without_output` → `test_load_config_without_io` (change `config.output` → `config.io`).

**Step 2: Run test to verify it fails**

Run: `cargo test test_load_config_with_io -- --nocapture`
Expected: FAIL — `IoConfig` doesn't exist, field `io` doesn't exist.

**Step 3: Implement config rename**

In `src/config.rs`:

Rename the struct at line 42-48:
```rust
/// I/O configuration for sandboxed file access in scripts.
#[derive(Debug, Clone, Deserialize)]
pub struct IoConfig {
    pub dir: Option<String>,
    pub max_bytes: Option<u64>,
    pub enabled: Option<bool>,
}
```

In `ToolScriptConfig` at line 62-71, rename field:
```rust
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
```

**Step 4: Update all references to the old names**

Files that reference `config::OutputConfig` or `config.output`:
- `src/main.rs:319-362` — `resolve_output_config` references `c.output` → `c.io`
- `src/main.rs` — rename `resolve_output_config` → `resolve_io_config`
- `src/executor.rs:14-20` — rename `OutputConfig` → `IoConfig` (this is a *different* struct — the resolved runtime config)

Since `config::OutputConfig` (TOML parse struct) and `executor::OutputConfig` (resolved runtime struct) are different types with the same name, rename both:
- `config::OutputConfig` → `config::IoConfig`
- `executor::OutputConfig` → `executor::IoConfig`

Update all imports and usages in:
- `src/main.rs` — imports, `resolve_output_config` → `resolve_io_config`, `output_config` vars
- `src/server/mod.rs:42` — `output_config: Option<OutputConfig>` → `io_config: Option<IoConfig>`
- `src/server/tools.rs` — references to output_config
- `src/runtime/executor.rs` — struct, field, constructor, execute method

**Step 5: Rename CLI flag**

In `src/cli.rs:46-48` and `src/cli.rs:79-81`:
```rust
/// I/O directory for sandboxed file access in scripts
#[arg(long)]
io_dir: Option<String>,
```

Update CLI tests at lines 218-242 to use `--io-dir`.

**Step 6: Run all tests**

Run: `cargo test`
Expected: PASS (all config, CLI, and existing tests pass with renamed types)

**Step 7: Commit**

```bash
git add -A && git commit -m "refactor: rename [output]/file.save to [io] config section"
```

---

### Task 2: Implement sandboxed file handle UserData

**Files:**
- Create: `src/runtime/io.rs`
- Modify: `src/runtime/mod.rs` (add `pub mod io;`)

This is the core implementation: a Rust struct that wraps `std::fs::File` and exposes `:read()`, `:write()`, `:lines()`, `:close()`, `:seek()`, `:flush()` as mlua UserData methods.

**Step 1: Write failing test**

In `src/runtime/io.rs`, add tests module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_handle_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let lua = mlua::Lua::new();
        let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
        register_io(&lua, io_ctx).unwrap();
        lua.sandbox(true).unwrap();

        let result: String = lua.load(r#"
            local f = io.open("test.txt", "w")
            f:write("hello world")
            f:close()
            local g = io.open("test.txt", "r")
            local content = g:read("*a")
            g:close()
            return content
        "#).eval().unwrap();

        assert_eq!(result, "hello world");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_file_handle_write_and_read -- --nocapture`
Expected: FAIL — `io` module doesn't exist.

**Step 3: Implement `IoContext` and `SandboxedFileHandle`**

Create `src/runtime/io.rs`:

```rust
use std::io::{BufRead, Read, Seek, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{FromLua, Lua, MultiValue, UserData, UserDataMethods, Value};

/// Maximum number of concurrently open file handles per execution.
const MAX_OPEN_HANDLES: usize = 64;

/// Tracks file operations for MCP response reporting.
#[derive(Debug, Clone)]
pub struct FileTouched {
    /// The relative filename as provided by the script.
    pub name: String,
    /// The resolved absolute path on disk.
    pub path: String,
    /// Operation: "write", "append", or "remove".
    pub op: String,
    /// Bytes on disk at end of execution (0 for remove).
    pub bytes: u64,
}

/// Shared I/O context for a single script execution.
#[derive(Clone)]
pub struct IoContext {
    root: PathBuf,
    max_bytes: u64,
    bytes_written: Arc<AtomicU64>,
    open_handles: Arc<AtomicUsize>,
    files_touched: Arc<Mutex<Vec<FileTouched>>>,
}
```

The `IoContext` struct holds the sandbox root, limits, and tracking state. It is cloned into each closure registered on the Lua VM.

Implement path validation as a method on `IoContext`:

```rust
impl IoContext {
    pub fn new(root: PathBuf, max_bytes: u64) -> Self {
        Self {
            root,
            max_bytes,
            bytes_written: Arc::new(AtomicU64::new(0)),
            open_handles: Arc::new(AtomicUsize::new(0)),
            files_touched: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Validate a path and resolve it to an absolute path within the sandbox.
    fn resolve(&self, filename: &str) -> Result<PathBuf, mlua::Error> {
        if filename.is_empty() {
            return Err(mlua::Error::external("path cannot be empty"));
        }
        if filename.contains('\0') {
            return Err(mlua::Error::external("path cannot contain null bytes"));
        }
        let path = std::path::Path::new(filename);
        if path.is_absolute() {
            return Err(mlua::Error::external("path must be relative"));
        }
        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(mlua::Error::external(
                    "path cannot contain '..' traversal",
                ));
            }
        }
        Ok(self.root.join(filename))
    }

    /// Track bytes written, enforcing the cumulative limit.
    fn track_write(&self, n: u64) -> Result<(), mlua::Error> {
        let prev = self.bytes_written.fetch_add(n, Ordering::SeqCst);
        if prev + n > self.max_bytes {
            self.bytes_written.fetch_sub(n, Ordering::SeqCst);
            return Err(mlua::Error::external(format!(
                "I/O write limit exceeded ({} bytes)", self.max_bytes
            )));
        }
        Ok(())
    }

    /// Increment open handle count, enforcing the limit.
    fn acquire_handle(&self) -> Result<(), mlua::Error> {
        let prev = self.open_handles.fetch_add(1, Ordering::SeqCst);
        if prev >= MAX_OPEN_HANDLES {
            self.open_handles.fetch_sub(1, Ordering::SeqCst);
            return Err(mlua::Error::external(format!(
                "too many open files (max {MAX_OPEN_HANDLES})"
            )));
        }
        Ok(())
    }

    fn release_handle(&self) {
        self.open_handles.fetch_sub(1, Ordering::SeqCst);
    }

    pub fn take_files_touched(&self) -> Vec<FileTouched> {
        self.files_touched.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
    }
}
```

Implement `SandboxedFileHandle` as UserData:

```rust
struct SandboxedFileHandle {
    file: Arc<Mutex<Option<std::fs::File>>>,
    ctx: IoContext,
    name: String,
    mode: String,
}

impl SandboxedFileHandle {
    fn with_file<T>(
        &self,
        f: impl FnOnce(&mut std::fs::File) -> Result<T, mlua::Error>,
    ) -> Result<T, mlua::Error> {
        let mut guard = self.file.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
        let file = guard.as_mut().ok_or_else(|| mlua::Error::external("attempt to use a closed file"))?;
        f(file)
    }
}

impl UserData for SandboxedFileHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // :read(fmt?) — "*a" (all), "*l" (line, default), "*n" (number)
        methods.add_method("read", |_, this, fmt: Option<String>| {
            let fmt = fmt.as_deref().unwrap_or("*l");
            this.with_file(|file| match fmt {
                "*a" | "*all" => {
                    let mut buf = String::new();
                    file.read_to_string(&mut buf).map_err(mlua::Error::external)?;
                    Ok(Value::String(/* lua string from buf */))
                    // Note: actual impl needs lua ref, use closure that captures lua
                }
                "*l" | "*line" => {
                    let mut reader = std::io::BufReader::new(file);
                    let mut line = String::new();
                    let n = reader.read_line(&mut line).map_err(mlua::Error::external)?;
                    if n == 0 { return Ok(Value::Nil); }
                    // trim trailing newline
                    if line.ends_with('\n') { line.pop(); }
                    if line.ends_with('\r') { line.pop(); }
                    Ok(Value::String(/* lua string */))
                }
                "*n" | "*number" => {
                    // read whitespace-delimited token and parse as number
                    // ...
                }
                _ => Err(mlua::Error::external(format!("invalid format: {fmt}"))),
            })
        });

        // :write(data...) — returns self for chaining
        methods.add_method("write", |_, this, args: MultiValue| {
            this.with_file(|file| {
                for arg in &args {
                    let s = match arg {
                        Value::String(s) => s.to_string_lossy().to_string(),
                        Value::Number(n) => n.to_string(),
                        Value::Integer(n) => n.to_string(),
                        _ => return Err(mlua::Error::external("write expects strings or numbers")),
                    };
                    this.ctx.track_write(s.len() as u64)?;
                    file.write_all(s.as_bytes()).map_err(mlua::Error::external)?;
                }
                Ok(())
            })?;
            // return self for chaining — handled by returning UserData
        });

        // :close()
        methods.add_method("close", |_, this, ()| {
            let mut guard = this.file.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
            if guard.is_none() {
                return Err(mlua::Error::external("attempt to close a closed file"));
            }
            *guard = None;
            this.ctx.release_handle();
            Ok(true)
        });

        // :seek(whence?, offset?)
        methods.add_method("seek", |_, this, (whence, offset): (Option<String>, Option<i64>)| {
            let whence = whence.as_deref().unwrap_or("cur");
            let offset = offset.unwrap_or(0);
            this.with_file(|file| {
                let pos = match whence {
                    "set" => file.seek(std::io::SeekFrom::Start(offset as u64)),
                    "cur" => file.seek(std::io::SeekFrom::Current(offset)),
                    "end" => file.seek(std::io::SeekFrom::End(offset)),
                    _ => return Err(mlua::Error::external(format!("invalid whence: {whence}"))),
                }.map_err(mlua::Error::external)?;
                Ok(pos as i64)
            })
        });

        // :flush()
        methods.add_method("flush", |_, this, ()| {
            this.with_file(|file| {
                file.flush().map_err(mlua::Error::external)?;
                Ok(())
            })
        });

        // :lines() — iterator
        methods.add_method("lines", |lua, this, ()| {
            // Return a Lua function that reads one line per call
            // returning nil when EOF is reached
        });
    }
}
```

Note: The code above is pseudocode showing the structure. The actual implementation needs to handle:
- Getting a `&Lua` reference in read methods to create Lua strings
- `:write()` returning the handle for chaining (return `AnyUserData` or use `add_method_mut`)
- `:lines()` creating a stateful iterator function
- BufReader positioning issues (use raw file + manual line reading, or store BufReader)

The implementer should use `methods.add_method("read", |lua, this, ...| { ... })` where `lua` is the first parameter to create `lua.create_string(&buf)`.

For `:write()` chaining, the method cannot directly return `self`. Instead, the Luau script pattern `io.open("f","w"):write(data):close()` won't chain unless `:write()` returns a UserData. One approach: wrap in `AnyUserData` and return, or accept that chaining requires the Lua-side variable pattern.

**Step 4: Run test to verify it passes**

Run: `cargo test test_file_handle_write_and_read -- --nocapture`
Expected: PASS

**Step 5: Add more unit tests**

```rust
#[test]
fn test_read_lines_iterator() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result: String = lua.load(r#"
        local f = io.open("lines.txt", "w")
        f:write("line1\nline2\nline3\n")
        f:close()
        local collected = {}
        for line in io.lines("lines.txt") do
            table.insert(collected, line)
        end
        return table.concat(collected, ",")
    "#).eval().unwrap();

    assert_eq!(result, "line1,line2,line3");
}

#[test]
fn test_seek() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result: String = lua.load(r#"
        local f = io.open("seek.txt", "w")
        f:write("hello world")
        f:close()
        local g = io.open("seek.txt", "r")
        g:seek("set", 6)
        local rest = g:read("*a")
        g:close()
        return rest
    "#).eval().unwrap();

    assert_eq!(result, "world");
}

#[test]
fn test_append_mode() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result: String = lua.load(r#"
        local f = io.open("app.txt", "w")
        f:write("hello")
        f:close()
        local g = io.open("app.txt", "a")
        g:write(" world")
        g:close()
        local h = io.open("app.txt", "r")
        local content = h:read("*a")
        h:close()
        return content
    "#).eval().unwrap();

    assert_eq!(result, "hello world");
}

#[test]
fn test_io_type() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result: String = lua.load(r#"
        local f = io.open("t.txt", "w")
        local t1 = io.type(f)
        f:close()
        local t2 = io.type(f)
        local t3 = io.type("not a file")
        return t1 .. "," .. (t2 or "nil") .. "," .. (t3 or "nil")
    "#).eval().unwrap();

    // io.type returns "file" for open, "closed file" for closed, nil for non-file
    // but nil concatenation errors in Lua, so we use `or "nil"`
    assert_eq!(result, "file,closed file,nil");
}
```

**Step 6: Run all tests**

Run: `cargo test -p toolscript -- io`
Expected: PASS

**Step 7: Commit**

```bash
git add src/runtime/io.rs src/runtime/mod.rs && git commit -m "feat: implement sandboxed io file handles with read/write/seek/close"
```

---

### Task 3: Implement `io.open()`, `io.lines()`, `io.list()`, and `os.remove()`

**Files:**
- Modify: `src/runtime/io.rs` (add `register_io()` public function)

**Step 1: Write failing tests**

```rust
#[test]
fn test_io_list() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result: String = lua.load(r#"
        io.open("aaa.txt", "w"):write("a"):close()
        io.open("bbb.txt", "w"):write("b"):close()
        local entries = io.list()
        table.sort(entries)
        return table.concat(entries, ",")
    "#).eval().unwrap();

    assert_eq!(result, "aaa.txt,bbb.txt");
}

#[test]
fn test_os_remove() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result: String = lua.load(r#"
        io.open("del.txt", "w"):write("delete me"):close()
        os.remove("del.txt")
        local ok, err = pcall(function() io.open("del.txt", "r") end)
        return tostring(ok)
    "#).eval().unwrap();

    assert_eq!(result, "false");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test test_io_list -- --nocapture`
Expected: FAIL

**Step 3: Implement `register_io()`**

The `register_io()` function creates the `io` table and `os.remove`:

```rust
/// Register the sandboxed `io` table and `os.remove()` on the Lua VM.
///
/// Must be called BEFORE `sandbox(true)`. Returns the IoContext
/// for collecting files_touched after execution.
pub fn register_io(lua: &Lua, ctx: IoContext) -> Result<(), mlua::Error> {
    let io_table = lua.create_table()?;

    // io.open(path, mode?)
    let ctx_clone = ctx.clone();
    let open_fn = lua.create_function(move |lua, (path, mode): (String, Option<String>)| {
        let mode = mode.as_deref().unwrap_or("r");
        let full_path = ctx_clone.resolve(&path)?;

        // Create parent dirs for write/append
        let is_write = mode.starts_with('w') || mode.starts_with('a');
        if is_write {
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent).map_err(mlua::Error::external)?;
            }
        }

        let file = match mode {
            "r" | "rb" => std::fs::File::open(&full_path).map_err(mlua::Error::external)?,
            "w" | "wb" => std::fs::File::create(&full_path).map_err(mlua::Error::external)?,
            "a" | "ab" => std::fs::OpenOptions::new()
                .create(true).append(true).open(&full_path)
                .map_err(mlua::Error::external)?,
            _ => return Err(mlua::Error::external(format!("invalid mode: {mode}"))),
        };

        ctx_clone.acquire_handle()?;

        let handle = SandboxedFileHandle {
            file: Arc::new(Mutex::new(Some(file))),
            ctx: ctx_clone.clone(),
            name: path,
            mode: mode.to_string(),
        };
        lua.create_userdata(handle)
    })?;
    io_table.set("open", open_fn)?;

    // io.lines(path) — convenience line iterator
    let ctx_clone = ctx.clone();
    let lines_fn = lua.create_function(move |lua, path: String| {
        // Open file, return iterator function
        let full_path = ctx_clone.resolve(&path)?;
        let file = std::fs::File::open(&full_path).map_err(mlua::Error::external)?;
        ctx_clone.acquire_handle()?;
        let reader = Arc::new(Mutex::new(std::io::BufReader::new(file)));
        let ctx_inner = ctx_clone.clone();
        let iter_fn = lua.create_function(move |lua, ()| {
            let mut reader = reader.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
            let mut line = String::new();
            let n = reader.read_line(&mut line).map_err(mlua::Error::external)?;
            if n == 0 {
                ctx_inner.release_handle();
                return Ok(Value::Nil);
            }
            if line.ends_with('\n') { line.pop(); }
            if line.ends_with('\r') { line.pop(); }
            Ok(Value::String(lua.create_string(&line)?))
        })?;
        Ok(iter_fn)
    })?;
    io_table.set("lines", lines_fn)?;

    // io.type(obj)
    let type_fn = lua.create_function(|_, value: Value| {
        match &value {
            Value::UserData(ud) => {
                if let Ok(handle) = ud.borrow::<SandboxedFileHandle>() {
                    let guard = handle.file.lock()
                        .map_err(|e| mlua::Error::external(e.to_string()))?;
                    if guard.is_some() {
                        Ok(Some("file".to_string()))
                    } else {
                        Ok(Some("closed file".to_string()))
                    }
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    })?;
    io_table.set("type", type_fn)?;

    // io.list(path?)
    let ctx_clone = ctx.clone();
    let list_fn = lua.create_function(move |lua, path: Option<String>| {
        let full_path = if let Some(p) = &path {
            ctx_clone.resolve(p)?
        } else {
            ctx_clone.root.clone()
        };
        let entries = std::fs::read_dir(&full_path).map_err(mlua::Error::external)?;
        let table = lua.create_table()?;
        let mut i = 1;
        for entry in entries {
            let entry = entry.map_err(mlua::Error::external)?;
            let name = entry.file_name().to_string_lossy().to_string();
            table.set(i, name)?;
            i += 1;
        }
        Ok(table)
    })?;
    io_table.set("list", list_fn)?;

    lua.globals().set("io", io_table)?;

    // os.remove(path) — add to existing os table
    let ctx_clone = ctx.clone();
    let remove_fn = lua.create_function(move |_, path: String| {
        let full_path = ctx_clone.resolve(&path)?;
        if full_path.is_dir() {
            return Err(mlua::Error::external("os.remove cannot delete directories"));
        }
        std::fs::remove_file(&full_path).map_err(mlua::Error::external)?;
        Ok(true)
    })?;
    // Get existing os table and add remove to it
    let os_table = lua.globals().get::<mlua::Table>("os")?;
    os_table.set("remove", remove_fn)?;

    Ok(())
}
```

**Step 4: Run all io tests**

Run: `cargo test -p toolscript -- io`
Expected: PASS

**Step 5: Commit**

```bash
git add src/runtime/io.rs && git commit -m "feat: add io.open, io.lines, io.list, io.type, os.remove"
```

---

### Task 4: Implement final-state file tracking

**Files:**
- Modify: `src/runtime/io.rs` — add tracking logic to `IoContext`

The MCP response needs `files_touched` showing final disk state. Track which files were opened for write/append and which were removed.

**Step 1: Write failing test**

```rust
#[test]
fn test_files_touched_final_state() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    let ctx_ref = io_ctx.clone();
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    lua.load(r#"
        local f = io.open("kept.txt", "w")
        f:write("data")
        f:close()
        local g = io.open("deleted.txt", "w")
        g:write("temp")
        g:close()
        os.remove("deleted.txt")
    "#).exec().unwrap();

    let touched = ctx_ref.collect_final_state();
    assert_eq!(touched.len(), 2);

    let kept = touched.iter().find(|f| f.name == "kept.txt").unwrap();
    assert_eq!(kept.op, "write");
    assert_eq!(kept.bytes, 4);

    let deleted = touched.iter().find(|f| f.name == "deleted.txt").unwrap();
    assert_eq!(deleted.op, "remove");
    assert_eq!(deleted.bytes, 0);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_files_touched_final_state -- --nocapture`
Expected: FAIL — `collect_final_state` doesn't exist.

**Step 3: Implement tracking**

In `IoContext`, track file operations as they happen (which files were written to, which were removed). At collection time, check disk to determine final state:

```rust
impl IoContext {
    /// Collect final state of all files touched during execution.
    /// Checks disk to determine: file exists → write/append with size,
    /// file doesn't exist → remove.
    pub fn collect_final_state(&self) -> Vec<FileTouched> {
        let touched = self.files_touched.lock()
            .map(|g| g.clone()).unwrap_or_default();

        // Deduplicate by name, check disk for final state
        let mut seen = std::collections::HashMap::new();
        for ft in &touched {
            seen.insert(ft.name.clone(), ft.path.clone());
        }

        seen.iter().map(|(name, path)| {
            let full = std::path::Path::new(path);
            if full.exists() {
                let bytes = std::fs::metadata(full)
                    .map(|m| m.len()).unwrap_or(0);
                FileTouched {
                    name: name.clone(),
                    path: path.clone(),
                    op: "write".to_string(),
                    bytes,
                }
            } else {
                FileTouched {
                    name: name.clone(),
                    path: path.clone(),
                    op: "remove".to_string(),
                    bytes: 0,
                }
            }
        }).collect()
    }
}
```

Record operations in `io.open` (for write/append modes), and `os.remove`:
- In `io.open`: when mode is `w`/`a`, push to `files_touched`
- In `os.remove`: push to `files_touched`

**Step 4: Run test to verify it passes**

Run: `cargo test test_files_touched_final_state -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/runtime/io.rs && git commit -m "feat: track files_touched with final-state aggregation"
```

---

### Task 5: Add sandbox security tests

**Files:**
- Modify: `src/runtime/io.rs` (add security tests)

**Step 1: Write security tests**

```rust
#[test]
fn test_rejects_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result = lua.load(r#"return io.open("../evil.txt", "w")"#).eval::<Value>();
    assert!(result.is_err());
}

#[test]
fn test_rejects_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result = lua.load(r#"return io.open("/etc/passwd", "r")"#).eval::<Value>();
    assert!(result.is_err());
}

#[test]
fn test_rejects_null_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result = lua.load(r#"return io.open("te\0st.txt", "w")"#).eval::<Value>();
    assert!(result.is_err());
}

#[test]
fn test_enforces_write_limit() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 10); // 10 byte limit
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result = lua.load(r#"
        local f = io.open("big.txt", "w")
        f:write("12345678901") -- 11 bytes > 10 limit
        f:close()
    "#).exec();
    assert!(result.is_err());
}

#[test]
fn test_enforces_handle_limit() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    // Open MAX_OPEN_HANDLES + 1 files
    let script = format!(
        r#"
        local handles = {{}}
        for i = 1, {} do
            handles[i] = io.open("f" .. i .. ".txt", "w")
        end
        "#,
        MAX_OPEN_HANDLES + 1
    );
    let result = lua.load(&script).exec();
    assert!(result.is_err());
}

#[test]
fn test_use_after_close() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result = lua.load(r#"
        local f = io.open("test.txt", "w")
        f:close()
        f:write("after close")
    "#).exec();
    assert!(result.is_err());
}

#[test]
fn test_os_remove_rejects_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result = lua.load(r#"os.remove("../evil.txt")"#).exec();
    assert!(result.is_err());
}

#[test]
fn test_os_remove_rejects_directories() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("subdir")).unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result = lua.load(r#"os.remove("subdir")"#).exec();
    assert!(result.is_err());
}

#[test]
fn test_io_list_rejects_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let lua = mlua::Lua::new();
    let io_ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
    register_io(&lua, io_ctx).unwrap();
    lua.sandbox(true).unwrap();

    let result = lua.load(r#"return io.list("..")"#).eval::<Value>();
    assert!(result.is_err());
}
```

**Step 2: Run tests**

Run: `cargo test -p toolscript -- io`
Expected: PASS (all security tests pass against existing implementation)

**Step 3: Commit**

```bash
git add src/runtime/io.rs && git commit -m "test: add sandbox security tests for io library"
```

---

### Task 6: Wire `register_io()` into executor, replace `register_file_save()`

**Files:**
- Modify: `src/runtime/executor.rs:12,14-20,42-53,55-62,134-139,177-188`
- Modify: `src/runtime/sandbox.rs:27-36,123-203` (remove `register_file_save` and `FileWritten`)
- Modify: `src/runtime/sandbox.rs:296-298` (update `test_sandbox_blocks_io` — io now exists!)

**Step 1: Write failing test**

Update `test_execute_file_save` in `src/runtime/executor.rs` (line 444) to use `io.open`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_execute_io() {
    let io_dir = tempfile::tempdir().unwrap();
    let executor = ScriptExecutor::new(
        empty_manifest(),
        Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
        ExecutorConfig::default(),
        Some(IoConfig {
            dir: io_dir.path().to_path_buf(),
            max_bytes: 50 * 1024 * 1024,
        }),
        Arc::new(McpClientManager::empty()),
    );
    let auth = AuthCredentialsMap::new();

    let result = executor
        .execute(
            r#"
        local f = io.open("test.json", "w")
        f:write('{"hello":"world"}')
        f:close()
        return "done"
    "#,
            &auth,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.result, serde_json::json!("done"));
    assert_eq!(result.files_touched.len(), 1);
    assert_eq!(result.files_touched[0].name, "test.json");
    assert_eq!(result.files_touched[0].op, "write");

    let content = std::fs::read_to_string(io_dir.path().join("test.json")).unwrap();
    assert_eq!(content, r#"{"hello":"world"}"#);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_execute_io -- --nocapture`
Expected: FAIL — `files_touched` field doesn't exist on `ExecutionResult`.

**Step 3: Update executor structs and wiring**

In `src/runtime/executor.rs`:

1. Change import from `sandbox::{FileWritten, ...}` to use `io::{IoContext, FileTouched}`
2. Rename `OutputConfig` → `IoConfig` (already done in Task 1)
3. Replace `ExecutionResult`:
```rust
pub struct ExecutionResult {
    pub result: serde_json::Value,
    pub logs: Vec<String>,
    pub files_touched: Vec<FileTouched>,
}
```
4. Remove `ExecutionStats` struct entirely
5. In `execute()`, replace the `register_file_save` block (lines 134-139) with:
```rust
let io_ctx = if let Some(ref io_config) = self.io_config {
    let ctx = IoContext::new(io_config.dir.clone(), io_config.max_bytes);
    register_io(sandbox.lua(), ctx.clone())?;
    Some(ctx)
} else {
    None
};
```
6. Replace files_written collection (lines 177-188) with:
```rust
let files_touched = io_ctx
    .map(|ctx| ctx.collect_final_state())
    .unwrap_or_default();

Ok(ExecutionResult {
    result: result_json,
    logs,
    files_touched,
})
```

In `src/runtime/sandbox.rs`:
- Remove `FileWritten` struct (lines 27-36)
- Remove `register_file_save()` method (lines 123-203)
- Remove all `file_save` tests (lines 376-482)
- Update `test_sandbox_blocks_io` (line 296): when io is NOT registered, `io` should still be nil. This test stays valid — it tests the default sandbox without io registered.

**Step 4: Run all tests**

Run: `cargo test`
Expected: PASS

**Step 5: Commit**

```bash
git add -A && git commit -m "feat: wire sandboxed io into executor, remove file.save"
```

---

### Task 7: Update MCP response format — remove stats, use files_touched

**Files:**
- Modify: `src/server/tools.rs:405-471` (tool description and response format)
- Modify: `src/server/mod.rs` (update server tests that check response)

**Step 1: Write failing test**

Update the existing server integration test that checks `files_written` in the response to expect `files_touched` and no `stats`.

**Step 2: Run test to verify it fails**

Run: `cargo test -p toolscript -- server`
Expected: FAIL

**Step 3: Update response format**

In `src/server/tools.rs`, update `execute_script_tool_def()` description (line 408-414):
```rust
"Execute a Luau script against the SDK. Auth comes from server-side configuration.\n\n\
 Returns a JSON object with:\n\
 - result: the script's return value (any JSON type)\n\
 - logs: array of strings captured from print() calls\n\
 - files_touched: array of { name, op, bytes } for files modified via io/os\n\n\
 On error, returns a text message prefixed with \"Script execution error:\".",
```

Update `execute_script_async()` response (lines 448-462):
```rust
let response = serde_json::json!({
    "result": exec_result.result,
    "logs": exec_result.logs,
    "files_touched": exec_result.files_touched.iter().map(|f| {
        serde_json::json!({
            "name": f.name,
            "op": f.op,
            "bytes": f.bytes,
        })
    }).collect::<Vec<_>>(),
});
```

Note: `stats` is removed entirely. `path` is not included in `files_touched` (the LLM doesn't need absolute paths — the io dir is server-side context).

**Step 4: Run all tests**

Run: `cargo test`
Expected: PASS

**Step 5: Commit**

```bash
git add -A && git commit -m "feat: update MCP response to files_touched, remove stats"
```

---

### Task 8: Update CLI flags and main.rs wiring

**Files:**
- Modify: `src/main.rs:319-362` (rename `resolve_output_config` → `resolve_io_config`, update defaults)
- Modify: `src/main.rs:90-94,199-203` (call sites)

**Step 1: Update `resolve_io_config`**

Change the function to use `[io]` config and `--io-dir` flag. Update the default directory from `"./toolscript-output"` to `"./toolscript-files"`:

```rust
fn resolve_io_config(
    cli_io_dir: Option<&str>,
    config: Option<&ToolScriptConfig>,
    is_hosted: bool,
) -> Option<IoConfig> {
    if is_hosted && cli_io_dir.is_none() {
        let explicitly_enabled = config
            .and_then(|c| c.io.as_ref())
            .and_then(|o| o.enabled)
            .unwrap_or(false);
        if !explicitly_enabled {
            return None;
        }
    }

    if cli_io_dir.is_none()
        && config
            .and_then(|c| c.io.as_ref())
            .and_then(|o| o.enabled)
            == Some(false)
    {
        return None;
    }

    let dir = cli_io_dir
        .map(PathBuf::from)
        .or_else(|| {
            config
                .and_then(|c| c.io.as_ref())
                .and_then(|o| o.dir.as_ref())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("./toolscript-files"));

    let max_bytes = config
        .and_then(|c| c.io.as_ref())
        .and_then(|o| o.max_bytes)
        .unwrap_or(50 * 1024 * 1024);

    Some(executor::IoConfig { dir, max_bytes })
}
```

Update call sites to pass `io_dir` instead of `output_dir`.

**Step 2: Run all tests**

Run: `cargo test`
Expected: PASS

**Step 3: Commit**

```bash
git add -A && git commit -m "refactor: rename resolve_output_config to resolve_io_config, default dir to toolscript-files"
```

---

### Task 9: Update integration test and E2E tests

**Files:**
- Modify: `tests/full_roundtrip.rs:280-315` (use `io.open` instead of `file.save`)
- Modify: `e2e/tests/conftest.py:315-349` (rename fixture, use `--io-dir`)
- Modify: `e2e/tests/test_stdio_scripts.py:239-276` (use `io.open`, check `files_touched`)

**Step 1: Update full_roundtrip.rs**

Replace the `file.save` script with `io.open`:
```rust
// Replace lines ~296-297:
let f = io.open("pets.csv", "w")
f:write(csv)
f:close()
local g = io.open("summary.json", "w")
g:write(json.encode({ count = #pets }))
g:close()
```

Replace assertion (line ~307):
```rust
assert_eq!(result.files_touched.len(), 2);
```

**Step 2: Update E2E conftest.py**

Rename fixture `mcp_output_session` → `mcp_io_session`, change `--output-dir` → `--io-dir`.

**Step 3: Update E2E test_stdio_scripts.py**

- Rename `test_file_save_writes_to_disk` → `test_io_write_to_disk`
- Update script to use `io.open`/`:write()`/`:close()`
- Check `files_touched` instead of `files_written`
- Rename `test_file_save_rejects_traversal` → `test_io_rejects_traversal`
- Update script to use `io.open("../evil.txt", "w")`

**Step 4: Run all tests**

Run: `cargo test && cd e2e && pytest`
Expected: PASS

**Step 5: Commit**

```bash
git add -A && git commit -m "test: update integration and E2E tests for sandboxed io"
```

---

### Task 10: Update README and clean up old design docs

**Files:**
- Modify: `README.md` — update any references to `file.save()`, `[output]`, `--output-dir`
- No changes to old design docs (they're historical records)

**Step 1: Search README for outdated references**

Look for: `file.save`, `output`, `--output-dir`, `toolscript-output`

**Step 2: Update references**

Replace `file.save()` examples with `io.open()`/`:write()`. Update config examples from `[output]` to `[io]`. Update CLI flag docs from `--output-dir` to `--io-dir`. Update default dir from `toolscript-output` to `toolscript-files`.

Also update the sandbox section: `io` is now conditionally available (not blocked), and the response format description should mention `files_touched` instead of `files_written`, and note that `stats` is no longer included.

**Step 3: Run all tests one final time**

Run: `cargo test && cd e2e && pytest`
Expected: PASS

**Step 4: Commit**

```bash
git add README.md && git commit -m "docs: update README for sandboxed io library"
```

---

Plan complete and saved to `docs/plans/2026-03-01-sandboxed-io-impl.md`. Two execution options:

**1. Subagent-Driven (this session)** — I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Parallel Session (separate)** — Open a new session with executing-plans, batch execution with checkpoints

Which approach?