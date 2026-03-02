use std::collections::HashMap;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, MultiValue, UserData, UserDataMethods, Value};

/// Maximum number of concurrently open file handles per execution.
const MAX_OPEN_HANDLES: usize = 64;

// ---------------------------------------------------------------------------
// IoContext — shared state for one execution
// ---------------------------------------------------------------------------

/// Shared state that tracks every file operation within a single script
/// execution, enforcing sandbox limits (write budget, handle count, path
/// validation).
#[derive(Clone)]
pub struct IoContext {
    root: PathBuf,
    max_bytes: u64,
    bytes_written: Arc<AtomicU64>,
    open_handles: Arc<AtomicUsize>,
    files_touched: Arc<Mutex<HashMap<String, PathBuf>>>,
}

impl IoContext {
    pub fn new(root: PathBuf, max_bytes: u64) -> Self {
        Self {
            root,
            max_bytes,
            bytes_written: Arc::new(AtomicU64::new(0)),
            open_handles: Arc::new(AtomicUsize::new(0)),
            files_touched: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Validate a user-supplied filename and resolve it to an absolute path
    /// within the sandbox root.  Rejects empty names, null bytes, absolute
    /// paths, and any `..` component.
    fn resolve(&self, filename: &str) -> Result<PathBuf, mlua::Error> {
        if filename.is_empty() {
            return Err(mlua::Error::external("filename cannot be empty"));
        }
        if filename.contains('\0') {
            return Err(mlua::Error::external("filename cannot contain null bytes"));
        }
        let path = Path::new(filename);
        if path.is_absolute() {
            return Err(mlua::Error::external(
                "filename must be relative, got absolute path",
            ));
        }
        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(mlua::Error::external(
                    "filename cannot contain '..' path traversal",
                ));
            }
        }
        Ok(self.root.join(filename))
    }

    /// Atomically account for `n` bytes of write.  Rolls back and returns an
    /// error if the cumulative budget would be exceeded.
    fn track_write(&self, n: u64) -> Result<(), mlua::Error> {
        let prev = self.bytes_written.fetch_add(n, Ordering::SeqCst);
        if prev + n > self.max_bytes {
            self.bytes_written.fetch_sub(n, Ordering::SeqCst);
            return Err(mlua::Error::external(format!(
                "output size limit exceeded ({} bytes)",
                self.max_bytes
            )));
        }
        Ok(())
    }

    /// Acquire a handle slot, enforcing the max-open limit.
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

    /// Release a previously-acquired handle slot.
    fn release_handle(&self) {
        self.open_handles.fetch_sub(1, Ordering::SeqCst);
    }

    /// Record that a file was touched (written, appended, or removed) so the
    /// caller can inspect final state after execution.
    fn record_touch(&self, name: &str, abs_path: &Path) {
        if let Ok(mut map) = self.files_touched.lock() {
            map.insert(name.to_string(), abs_path.to_path_buf());
        }
    }

    /// After execution, inspect the disk to determine what happened to every
    /// file the script touched.
    pub fn collect_final_state(&self) -> Vec<FileTouched> {
        let Ok(map) = self.files_touched.lock() else {
            return Vec::new();
        };
        let mut result = Vec::with_capacity(map.len());
        for (name, abs_path) in &*map {
            let path_str = abs_path.to_string_lossy().to_string();
            if abs_path.exists() {
                let bytes = abs_path.metadata().map(|m| m.len()).unwrap_or(0);
                result.push(FileTouched {
                    name: name.clone(),
                    path: path_str,
                    op: "write".to_string(),
                    bytes,
                });
            } else {
                result.push(FileTouched {
                    name: name.clone(),
                    path: path_str,
                    op: "remove".to_string(),
                    bytes: 0,
                });
            }
        }
        drop(map);
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }
}

// ---------------------------------------------------------------------------
// FileTouched
// ---------------------------------------------------------------------------

/// Summary of a single file that was touched during script execution.
#[derive(Debug, Clone)]
pub struct FileTouched {
    pub name: String,
    pub path: String,
    pub op: String, // "write" or "remove"
    pub bytes: u64,
}

// ---------------------------------------------------------------------------
// SandboxedFileHandle — mlua UserData
// ---------------------------------------------------------------------------

/// A file handle exposed to Lua as a userdata value.  Wraps a raw
/// `std::fs::File` behind a mutex so that `:close()` can set it to `None`.
struct SandboxedFileHandle {
    inner: Arc<Mutex<Option<std::fs::File>>>,
    ctx: IoContext,
}

impl SandboxedFileHandle {
    #[allow(clippy::option_if_let_else)] // match is clearer here
    fn with_file<T, F>(&self, action: F) -> Result<T, mlua::Error>
    where
        F: FnOnce(&mut std::fs::File) -> Result<T, mlua::Error>,
    {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| mlua::Error::external(format!("lock poisoned: {e}")))?;
        match guard.as_mut() {
            Some(f) => action(f),
            None => Err(mlua::Error::external("attempt to use a closed file")),
        }
    }

    fn is_closed(&self) -> bool {
        self.inner.lock().map(|g| g.is_none()).unwrap_or(true)
    }
}

/// Read one line from a raw File, byte-by-byte.  Returns `None` only at true
/// EOF (zero bytes available).  Otherwise returns the line content WITHOUT the
/// trailing `\n`.
fn read_line(file: &mut std::fs::File) -> Result<Option<String>, mlua::Error> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    let mut got_any = false;
    loop {
        match file.read(&mut byte) {
            Ok(0) => break, // EOF
            Ok(_) => {
                got_any = true;
                if byte[0] == b'\n' {
                    break;
                }
                buf.push(byte[0]);
            }
            Err(e) => return Err(mlua::Error::external(e)),
        }
    }
    if !got_any {
        return Ok(None); // true EOF
    }
    // Strip trailing \r for Windows-style line endings
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(mlua::Error::external)
}

impl UserData for SandboxedFileHandle {
    #[allow(clippy::too_many_lines)]
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // :read(fmt?)
        methods.add_method("read", |lua, this, fmt: Value| {
            let fmt_str = match &fmt {
                Value::String(s) => s.to_string_lossy(),
                _ => "*l".to_string(),
            };
            this.with_file(|file| match fmt_str.as_str() {
                "*a" | "a" => {
                    let mut contents = String::new();
                    file.read_to_string(&mut contents)
                        .map_err(mlua::Error::external)?;
                    Ok(Value::String(lua.create_string(&contents)?))
                }
                "*l" | "l" => match read_line(file)? {
                    Some(line) => Ok(Value::String(lua.create_string(&line)?)),
                    None => Ok(Value::Nil),
                },
                "*n" | "n" => {
                    // Read whitespace, then a number token
                    let mut num_str = String::new();
                    let mut byte = [0u8; 1];
                    // Skip leading whitespace
                    loop {
                        match file.read(&mut byte) {
                            Ok(0) => return Ok(Value::Nil),
                            Ok(_) => {
                                if !byte[0].is_ascii_whitespace() {
                                    num_str.push(byte[0] as char);
                                    break;
                                }
                            }
                            Err(e) => return Err(mlua::Error::external(e)),
                        }
                    }
                    // Read digits/dots/sign/e
                    loop {
                        match file.read(&mut byte) {
                            Ok(0) => break,
                            Ok(_) => {
                                let c = byte[0] as char;
                                if c.is_ascii_digit()
                                    || c == '.'
                                    || c == '-'
                                    || c == '+'
                                    || c == 'e'
                                    || c == 'E'
                                {
                                    num_str.push(c);
                                } else {
                                    // Put back — we can't really unseek
                                    // easily, so seek back 1 byte
                                    let _ = file.seek(std::io::SeekFrom::Current(-1));
                                    break;
                                }
                            }
                            Err(e) => return Err(mlua::Error::external(e)),
                        }
                    }
                    num_str
                        .parse::<f64>()
                        .map_or(Ok(Value::Nil), |n| Ok(Value::Number(n)))
                }
                _ => Err(mlua::Error::external(format!(
                    "invalid format argument to read: '{fmt_str}'"
                ))),
            })
        });

        // :write(data...) — use add_function so we can return the AnyUserData
        methods.add_function(
            "write",
            |_lua, (this_ud, args): (AnyUserData, MultiValue)| {
                let this = this_ud.borrow::<Self>()?;
                this.with_file(|file| {
                    for arg in &args {
                        let data = match arg {
                            Value::String(s) => {
                                s.to_str().map_err(mlua::Error::external)?.to_string()
                            }
                            Value::Number(n) => n.to_string(),
                            Value::Integer(n) => n.to_string(),
                            _ => {
                                return Err(mlua::Error::external(
                                    "write expects string or number arguments",
                                ));
                            }
                        };
                        let bytes = data.as_bytes();
                        this.ctx.track_write(bytes.len() as u64)?;
                        file.write_all(bytes).map_err(mlua::Error::external)?;
                    }
                    Ok(())
                })?;
                drop(this);
                Ok(this_ud)
            },
        );

        // :close()
        methods.add_method("close", |_lua, this, ()| {
            let mut guard = this
                .inner
                .lock()
                .map_err(|e| mlua::Error::external(format!("lock poisoned: {e}")))?;
            if guard.is_none() {
                return Err(mlua::Error::external("attempt to use a closed file"));
            }
            *guard = None;
            drop(guard);
            this.ctx.release_handle();
            Ok(true)
        });

        // :seek(whence?, offset?)
        methods.add_method(
            "seek",
            |_lua, this, (whence, offset): (Option<String>, Option<i64>)| {
                let whence_str = whence.unwrap_or_else(|| "cur".to_string());
                let offset_val = offset.unwrap_or(0);
                this.with_file(|file| {
                    let pos = match whence_str.as_str() {
                        "set" => std::io::SeekFrom::Start(
                            u64::try_from(offset_val)
                                .map_err(|_| mlua::Error::external("invalid offset for 'set'"))?,
                        ),
                        "cur" => std::io::SeekFrom::Current(offset_val),
                        "end" => std::io::SeekFrom::End(offset_val),
                        _ => {
                            return Err(mlua::Error::external(format!(
                                "invalid whence argument: '{whence_str}'"
                            )));
                        }
                    };
                    let new_pos = file.seek(pos).map_err(mlua::Error::external)?;
                    #[allow(clippy::cast_precision_loss)]
                    Ok(new_pos as f64)
                })
            },
        );

        // :flush()
        methods.add_method("flush", |_lua, this, ()| {
            this.with_file(|file| {
                file.flush().map_err(mlua::Error::external)?;
                Ok(true)
            })
        });

        // :lines() — return an iterator function
        methods.add_method("lines", |lua, this, ()| {
            // Clone the inner Arc so the iterator can outlive the borrow
            let inner = Arc::clone(&this.inner);
            let iter_fn = lua.create_function(move |lua, ()| {
                let mut guard = inner
                    .lock()
                    .map_err(|e| mlua::Error::external(format!("lock poisoned: {e}")))?;
                match guard.as_mut() {
                    Some(file) => match read_line(file)? {
                        Some(line) => Ok(Value::String(lua.create_string(&line)?)),
                        None => Ok(Value::Nil),
                    },
                    None => Err(mlua::Error::external("attempt to use a closed file")),
                }
            })?;
            Ok(iter_fn)
        });
    }
}

// ---------------------------------------------------------------------------
// register_io() — wire everything into Lua globals
// ---------------------------------------------------------------------------

/// Register the sandboxed `io` table and `os.remove` into the Lua state.
///
/// Must be called **before** `lua.sandbox(true)`.
#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
pub fn register_io(lua: &Lua, ctx: IoContext) -> Result<(), mlua::Error> {
    let io_table = lua.create_table()?;

    // -- io.open(path, mode?) -----------------------------------------------
    {
        let ctx = ctx.clone();
        let open_fn = lua.create_function(move |lua, (path, mode): (String, Option<String>)| {
            let mode_str = mode.unwrap_or_else(|| "r".to_string());
            let abs_path = ctx.resolve(&path)?;

            let file = match mode_str.as_str() {
                "r" | "rb" => std::fs::File::open(&abs_path).map_err(mlua::Error::external)?,
                "w" | "wb" => {
                    if let Some(parent) = abs_path.parent() {
                        std::fs::create_dir_all(parent).map_err(mlua::Error::external)?;
                    }
                    ctx.record_touch(&path, &abs_path);
                    std::fs::File::create(&abs_path).map_err(mlua::Error::external)?
                }
                "a" | "ab" => {
                    if let Some(parent) = abs_path.parent() {
                        std::fs::create_dir_all(parent).map_err(mlua::Error::external)?;
                    }
                    ctx.record_touch(&path, &abs_path);
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&abs_path)
                        .map_err(mlua::Error::external)?
                }
                _ => {
                    return Err(mlua::Error::external(format!("invalid mode: '{mode_str}'")));
                }
            };

            ctx.acquire_handle()?;

            let handle = SandboxedFileHandle {
                inner: Arc::new(Mutex::new(Some(file))),
                ctx: ctx.clone(),
            };
            lua.create_userdata(handle)
        })?;
        io_table.set("open", open_fn)?;
    }

    // -- io.lines(path) -----------------------------------------------------
    {
        let ctx = ctx.clone();
        let lines_fn = lua.create_function(move |lua, path: String| {
            let abs_path = ctx.resolve(&path)?;
            let file = std::fs::File::open(&abs_path).map_err(mlua::Error::external)?;
            ctx.acquire_handle()?;
            let inner: Arc<Mutex<Option<std::fs::File>>> = Arc::new(Mutex::new(Some(file)));
            let ctx_clone = ctx.clone();
            let iter_fn = lua.create_function(move |lua, ()| {
                let mut guard = inner
                    .lock()
                    .map_err(|e| mlua::Error::external(format!("lock poisoned: {e}")))?;
                let Some(file) = guard.as_mut() else {
                    return Ok(Value::Nil);
                };
                if let Some(line) = read_line(file)? {
                    Ok(Value::String(lua.create_string(&line)?))
                } else {
                    // Auto-close at EOF
                    *guard = None;
                    drop(guard);
                    ctx_clone.release_handle();
                    Ok(Value::Nil)
                }
            })?;
            Ok(iter_fn)
        })?;
        io_table.set("lines", lines_fn)?;
    }

    // -- io.type(obj) -------------------------------------------------------
    {
        let type_fn = lua.create_function(|lua_inner, val: Value| {
            match val {
                Value::UserData(ud) => {
                    // Try to borrow as SandboxedFileHandle
                    match ud.borrow::<SandboxedFileHandle>() {
                        Ok(handle) => {
                            if handle.is_closed() {
                                Ok(Value::String(lua_inner.create_string("closed file")?))
                            } else {
                                Ok(Value::String(lua_inner.create_string("file")?))
                            }
                        }
                        Err(_) => Ok(Value::Nil),
                    }
                }
                _ => Ok(Value::Nil),
            }
        })?;
        io_table.set("type", type_fn)?;
    }

    // -- io.list(path?) -----------------------------------------------------
    {
        let ctx = ctx.clone();
        let list_fn = lua.create_function(move |lua, path: Option<String>| {
            let abs_dir = match path {
                Some(p) => ctx.resolve(&p)?,
                None => ctx.root.clone(),
            };
            if !abs_dir.is_dir() {
                return Err(mlua::Error::external(format!(
                    "'{}' is not a directory",
                    abs_dir.display()
                )));
            }
            let entries = std::fs::read_dir(&abs_dir).map_err(mlua::Error::external)?;
            let result = lua.create_table()?;
            let mut idx = 1i64;
            for entry in entries {
                let entry = entry.map_err(mlua::Error::external)?;
                let name = entry.file_name().to_string_lossy().to_string();
                result.set(idx, name)?;
                idx += 1;
            }
            Ok(result)
        })?;
        io_table.set("list", list_fn)?;
    }

    lua.globals().set("io", io_table)?;

    // -- os.remove(path) — add to existing os table -------------------------
    {
        let remove_fn = lua.create_function(move |_lua, path: String| {
            let abs_path = ctx.resolve(&path)?;
            if abs_path.is_dir() {
                return Err(mlua::Error::external(format!(
                    "cannot remove directory '{path}'"
                )));
            }
            std::fs::remove_file(&abs_path).map_err(mlua::Error::external)?;
            ctx.record_touch(&path, &abs_path);
            Ok(true)
        })?;

        // Get the existing os table or create a new one
        let os_table: mlua::Table = if let Ok(t) = lua.globals().get::<mlua::Table>("os") {
            t
        } else {
            let t = lua.create_table()?;
            lua.globals().set("os", t.clone())?;
            t
        };
        os_table.set("remove", remove_fn)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn setup() -> (tempfile::TempDir, Lua) {
        let dir = tempfile::tempdir().unwrap();
        let lua = Lua::new();
        let ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
        register_io(&lua, ctx).unwrap();
        lua.sandbox(true).unwrap();
        (dir, lua)
    }

    fn setup_with_limit(max_bytes: u64) -> (tempfile::TempDir, Lua, IoContext) {
        let dir = tempfile::tempdir().unwrap();
        let lua = Lua::new();
        let ctx = IoContext::new(dir.path().to_path_buf(), max_bytes);
        register_io(&lua, ctx.clone()).unwrap();
        lua.sandbox(true).unwrap();
        (dir, lua, ctx)
    }

    // Basic write + read roundtrip
    #[test]
    fn test_write_and_read_all() {
        let (_dir, lua) = setup();
        let result: String = lua
            .load(
                r#"
                local f = io.open("hello.txt", "w")
                f:write("hello world")
                f:close()
                local f2 = io.open("hello.txt", "r")
                local content = f2:read("*a")
                f2:close()
                return content
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, "hello world");
    }

    // Read *l (line by line)
    #[test]
    fn test_read_line() {
        let (dir, lua) = setup();
        std::fs::write(dir.path().join("lines.txt"), "line1\nline2\nline3\n").unwrap();
        let result: String = lua
            .load(
                r#"
                local f = io.open("lines.txt", "r")
                local a = f:read("*l")
                local b = f:read("*l")
                local c = f:read("*l")
                f:close()
                return a .. "|" .. b .. "|" .. c
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, "line1|line2|line3");
    }

    // Read *n (number)
    #[test]
    fn test_read_number() {
        let (dir, lua) = setup();
        std::fs::write(dir.path().join("nums.txt"), "  42  3.14").unwrap();
        let result: f64 = lua
            .load(
                r#"
                local f = io.open("nums.txt", "r")
                local a = f:read("*n")
                local b = f:read("*n")
                f:close()
                return a + b
                "#,
            )
            .eval()
            .unwrap();
        assert!((result - 45.14).abs() < 0.001);
    }

    // io.lines() iterator
    #[test]
    fn test_io_lines() {
        let (dir, lua) = setup();
        std::fs::write(dir.path().join("iter.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let result: String = lua
            .load(
                r#"
                local parts = {}
                for line in io.lines("iter.txt") do
                    table.insert(parts, line)
                end
                return table.concat(parts, ",")
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, "alpha,beta,gamma");
    }

    // handle:lines() iterator
    #[test]
    fn test_handle_lines() {
        let (dir, lua) = setup();
        std::fs::write(dir.path().join("hlines.txt"), "one\ntwo\nthree\n").unwrap();
        let result: String = lua
            .load(
                r#"
                local f = io.open("hlines.txt", "r")
                local parts = {}
                for line in f:lines() do
                    table.insert(parts, line)
                end
                f:close()
                return table.concat(parts, ",")
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, "one,two,three");
    }

    // Seek
    #[test]
    fn test_seek() {
        let (_dir, lua) = setup();
        let result: String = lua
            .load(
                r#"
                local f = io.open("seek.txt", "w")
                f:write("abcdefghij")
                f:close()
                local f2 = io.open("seek.txt", "r")
                f2:seek("set", 3)
                local data = f2:read("*a")
                f2:close()
                return data
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, "defghij");
    }

    // Append mode
    #[test]
    fn test_append_mode() {
        let (_dir, lua) = setup();
        let result: String = lua
            .load(
                r#"
                local f = io.open("app.txt", "w")
                f:write("hello")
                f:close()
                local f2 = io.open("app.txt", "a")
                f2:write(" world")
                f2:close()
                local f3 = io.open("app.txt", "r")
                local data = f3:read("*a")
                f3:close()
                return data
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, "hello world");
    }

    // io.type()
    #[test]
    fn test_io_type() {
        let (_dir, lua) = setup();
        let result: String = lua
            .load(
                r#"
                local f = io.open("t.txt", "w")
                f:write("x")
                local t1 = io.type(f)
                f:close()
                local t2 = io.type(f)
                local t3 = io.type("not a file")
                local t3_str = tostring(t3)
                return t1 .. "|" .. t2 .. "|" .. t3_str
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, "file|closed file|nil");
    }

    // io.list()
    #[test]
    fn test_io_list() {
        let (dir, lua) = setup();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        let result: Vec<String> = lua
            .load(
                r#"
                local entries = io.list()
                table.sort(entries)
                return entries
                "#,
            )
            .eval::<mlua::Table>()
            .unwrap()
            .sequence_values::<String>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(result, vec!["a.txt", "b.txt"]);
    }

    // io.list() with subdirectory
    #[test]
    fn test_io_list_subdir() {
        let (dir, lua) = setup();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/c.txt"), "").unwrap();
        std::fs::write(dir.path().join("sub/d.txt"), "").unwrap();
        let result: Vec<String> = lua
            .load(
                r#"
                local entries = io.list("sub")
                table.sort(entries)
                return entries
                "#,
            )
            .eval::<mlua::Table>()
            .unwrap()
            .sequence_values::<String>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(result, vec!["c.txt", "d.txt"]);
    }

    // os.remove()
    #[test]
    fn test_os_remove() {
        let (dir, lua) = setup();
        std::fs::write(dir.path().join("gone.txt"), "bye").unwrap();
        let result: bool = lua
            .load(
                r#"
                os.remove("gone.txt")
                return true
                "#,
            )
            .eval()
            .unwrap();
        assert!(result);
        assert!(!dir.path().join("gone.txt").exists());
    }

    // Write chaining: f:write("a"):write("b"):close()
    #[test]
    fn test_write_chaining() {
        let (_dir, lua) = setup();
        let result: String = lua
            .load(
                r#"
                local f = io.open("chain.txt", "w")
                f:write("hello"):write(" "):write("world")
                f:close()
                local f2 = io.open("chain.txt", "r")
                local data = f2:read("*a")
                f2:close()
                return data
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, "hello world");
    }

    // flush()
    #[test]
    fn test_flush() {
        let (dir, lua) = setup();
        let result: bool = lua
            .load(
                r#"
                local f = io.open("fl.txt", "w")
                f:write("flushed")
                f:flush()
                f:close()
                return true
                "#,
            )
            .eval()
            .unwrap();
        assert!(result);
        let content = std::fs::read_to_string(dir.path().join("fl.txt")).unwrap();
        assert_eq!(content, "flushed");
    }

    // Subdirectory auto-creation on write
    #[test]
    fn test_subdirectory_auto_create() {
        let (dir, lua) = setup();
        let result: String = lua
            .load(
                r#"
                local f = io.open("deep/nested/file.txt", "w")
                f:write("deep content")
                f:close()
                local f2 = io.open("deep/nested/file.txt", "r")
                local data = f2:read("*a")
                f2:close()
                return data
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, "deep content");
        assert!(dir.path().join("deep/nested/file.txt").exists());
    }

    // Security: path traversal
    #[test]
    fn test_rejects_path_traversal() {
        let (_dir, lua) = setup();
        let result = lua
            .load(r#"return io.open("../evil.txt", "w")"#)
            .eval::<Value>();
        assert!(result.is_err());
    }

    // Security: absolute path
    #[test]
    fn test_rejects_absolute_path() {
        let (_dir, lua) = setup();
        let result = lua
            .load(r#"return io.open("/etc/passwd", "r")"#)
            .eval::<Value>();
        assert!(result.is_err());
    }

    // Security: null bytes
    #[test]
    fn test_rejects_null_bytes() {
        let (_dir, lua) = setup();
        let result = lua
            .load(r#"return io.open("te\0st.txt", "w")"#)
            .eval::<Value>();
        assert!(result.is_err());
    }

    // Security: write limit
    #[test]
    fn test_enforces_write_limit() {
        let (_dir, lua, _ctx) = setup_with_limit(10);
        // Write 5 bytes — should succeed
        lua.load(
            r#"
            local f = io.open("a.txt", "w")
            f:write("hello")
            f:close()
            "#,
        )
        .exec()
        .unwrap();
        // Write 6 more bytes — should fail (total 11 > 10)
        let result = lua
            .load(
                r#"
                local f = io.open("b.txt", "w")
                f:write("world!")
                f:close()
                "#,
            )
            .exec();
        assert!(result.is_err());
    }

    // Security: handle limit
    #[test]
    fn test_enforces_handle_limit() {
        let dir = tempfile::tempdir().unwrap();
        let lua = Lua::new();
        let ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
        // Pre-set the handle counter to MAX_OPEN_HANDLES - 1
        ctx.open_handles
            .store(MAX_OPEN_HANDLES - 1, Ordering::SeqCst);
        register_io(&lua, ctx).unwrap();
        lua.sandbox(true).unwrap();

        // First open should succeed (hits MAX_OPEN_HANDLES)
        std::fs::write(dir.path().join("x.txt"), "data").unwrap();
        lua.load(
            r#"
            local f = io.open("x.txt", "r")
            "#,
        )
        .exec()
        .unwrap();

        // Second open should fail (exceeds MAX_OPEN_HANDLES)
        let result = lua
            .load(
                r#"
                local f2 = io.open("x.txt", "r")
                "#,
            )
            .exec();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too many open files"), "error was: {err}");
    }

    // Error: use after close
    #[test]
    fn test_use_after_close() {
        let (_dir, lua) = setup();
        let result = lua
            .load(
                r#"
                local f = io.open("uc.txt", "w")
                f:write("data")
                f:close()
                f:write("more")
                "#,
            )
            .exec();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("closed file"), "error was: {err}");
    }

    // Error: os.remove rejects traversal
    #[test]
    fn test_os_remove_rejects_traversal() {
        let (_dir, lua) = setup();
        let result = lua.load(r#"os.remove("../evil.txt")"#).exec();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("traversal"), "error was: {err}");
    }

    // Error: os.remove rejects directories
    #[test]
    fn test_os_remove_rejects_directories() {
        let (dir, lua) = setup();
        std::fs::create_dir_all(dir.path().join("mydir")).unwrap();
        let result = lua.load(r#"os.remove("mydir")"#).exec();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cannot remove directory"), "error was: {err}");
    }

    // Error: io.list rejects traversal
    #[test]
    fn test_io_list_rejects_traversal() {
        let (_dir, lua) = setup();
        let result = lua.load(r#"return io.list("..")"#).eval::<Value>();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("traversal"), "error was: {err}");
    }

    // Final state tracking
    #[test]
    fn test_collect_final_state() {
        let dir = tempfile::tempdir().unwrap();
        let lua = Lua::new();
        let ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
        let ctx_ref = ctx.clone();
        register_io(&lua, ctx).unwrap();
        lua.sandbox(true).unwrap();

        lua.load(
            r#"
            local f = io.open("state.txt", "w")
            f:write("some data")
            f:close()
            "#,
        )
        .exec()
        .unwrap();

        let state = ctx_ref.collect_final_state();
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].name, "state.txt");
        assert_eq!(state[0].op, "write");
        assert_eq!(state[0].bytes, 9);
    }

    // Final state: write then delete shows remove
    #[test]
    fn test_final_state_write_then_delete() {
        let dir = tempfile::tempdir().unwrap();
        let lua = Lua::new();
        let ctx = IoContext::new(dir.path().to_path_buf(), 50 * 1024 * 1024);
        let ctx_ref = ctx.clone();
        register_io(&lua, ctx).unwrap();
        lua.sandbox(true).unwrap();

        lua.load(
            r#"
            local f = io.open("del.txt", "w")
            f:write("temporary")
            f:close()
            os.remove("del.txt")
            "#,
        )
        .exec()
        .unwrap();

        let state = ctx_ref.collect_final_state();
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].name, "del.txt");
        assert_eq!(state[0].op, "remove");
        assert_eq!(state[0].bytes, 0);
    }
}
