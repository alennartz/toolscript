# Sandboxed Luau `io` Library

**Date:** 2026-03-01
**Status:** Approved

## Overview

Replace the current `file.save()` / `[output]` system with a Rust-backed
implementation of the standard Lua `io` library, sandboxed to a configurable
directory. Scripts get familiar Lua file I/O syntax; the Rust layer constrains
all operations to a single directory with no escape.

Both stdio and hosted modes use the same implementation. Stdio defaults to
enabled; hosted defaults to disabled (same posture as today).

## Config

```toml
[io]
dir = "./toolscript-files"    # relative to CWD, no ".." allowed
max_bytes = 52428800           # cumulative write limit per execution (50MB)
enabled = true                 # default: true for stdio, false for hosted
```

CLI flag: `--io-dir <path>` (replaces `--output-dir`).

## API Surface

### `io` table (registered as Luau global)

| Function              | Standard Lua? | Description                                      |
|-----------------------|---------------|--------------------------------------------------|
| `io.open(path, mode?)`| Yes          | Modes: `r` (default), `w`, `a`, `rb`, `wb`, `ab` |
| `io.lines(path)`      | Yes          | Line iterator convenience                         |
| `io.type(obj)`        | Yes          | `"file"` / `"closed file"` / `nil`               |
| `io.list(path?)`      | No (custom)  | Returns `{string}` of entry names, no recursion   |

### File handle methods (UserData)

| Method                         | Standard Lua? | Description                        |
|--------------------------------|---------------|------------------------------------|
| `handle:read(fmt?)`            | Yes           | `"*a"`, `"*l"` (default), `"*n"`  |
| `handle:write(data...)`        | Yes           | Returns handle for chaining        |
| `handle:lines()`               | Yes           | Line iterator on handle            |
| `handle:close()`               | Yes           | Returns `true`                     |
| `handle:seek(whence?, offset?)`| Yes           | `"set"`, `"cur"`, `"end"`         |
| `handle:flush()`               | Yes           | Flushes write buffer               |

### `os.remove(path)` (added to existing `os` table)

| Function          | Standard Lua? | Description                     |
|-------------------|---------------|---------------------------------|
| `os.remove(path)` | Yes          | Deletes a file (not directories)|

### Not implemented

- `io.popen`, `io.stdin`, `io.stdout`, `io.stderr`, `io.tmpfile` — process I/O
- `io.input()`, `io.output()`, `io.read()`, `io.write()`, `io.flush()` — default-file shortcuts
- `handle:setvbuf()` — buffering control

## Sandboxing Rules

- **Relative paths only** — absolute paths rejected
- **No `..`** — path traversal rejected
- **No null bytes** — C string injection rejected
- **Single directory** — all operations confined to configured `io.dir`
- **Cumulative write limit** — tracked via `AtomicU64`, same as today
- **Max 64 concurrent open handles** — prevents file descriptor exhaustion
- **Auto-create parent dirs** — `io.open("sub/dir/f.csv", "w")` creates `sub/dir/`
- **Auto-close on VM teardown** — handles backed by `Arc<Mutex<File>>`, dropped
  when VM is destroyed. No script cooperation needed for cleanup.
- **Fresh VM per execution** — no state leaks across runs

## Mode Behavior

| Mode               | Default      | Can be enabled                          |
|--------------------|--------------|-----------------------------------------|
| Stdio (local)      | `io` enabled | Always                                  |
| Hosted (JWT auth)  | `io` disabled| Via `--io-dir` or `[io] enabled = true` |

## MCP Response Changes

### `files_touched` replaces `files_written`

Aggregation uses **final state**: after script execution, check what's on disk.
Files that exist are reported as `write`/`append` with total bytes. Files that
were deleted are reported as `remove`. Intermediate operations are not reported.

```json
{
  "result": "done",
  "logs": ["Processing 10000 rows", "Done"],
  "files_touched": [
    { "name": "output.csv", "op": "write", "bytes": 524288 },
    { "name": "temp.txt", "op": "remove" }
  ]
}
```

`logs` (from `print()`) is unchanged — stays buffered and returned in the
response so the LLM reliably sees it.

### `stats` field removed

The `stats` object (`api_calls`, `duration_ms`) is removed from the MCP tool
response. It should not be passed back to the LLM.

## Migration from `file.save()`

| Before                           | After                                               |
|----------------------------------|-----------------------------------------------------|
| `[output]` config section        | `[io]`                                              |
| `--output-dir` CLI flag          | `--io-dir`                                          |
| `file.save(name, content)`       | `io.open(name, "w"):write(content):close()`         |
| Default dir `./toolscript-output`| Default dir `./toolscript-files`                    |
| `FileWritten` struct             | `FileTouched` with `op` field                       |
| `files_written` in response      | `files_touched` in response                         |
| `stats` in response              | Removed                                             |

## Example Usage

```lua
-- Write a CSV
local out = io.open("results.csv", "w")
out:write("id,name,total\n")
for _, row in ipairs(data) do
    out:write(row.id .. "," .. row.name .. "," .. row.total .. "\n")
end
out:close()

-- Read it back and process
local f = io.open("results.csv", "r")
local content = f:read("*a")
f:close()

-- Line-by-line iteration
for line in io.lines("results.csv") do
    print(line)
end

-- List directory contents
local files = io.list(".")
for _, name in ipairs(files) do
    print(name)
end

-- Clean up temp files
os.remove("temp.txt")
```
