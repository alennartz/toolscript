# Design: `file.save()` — Safe Disk Output from Luau Scripts

## Problem

LLMs using code-mcp need to persist data from Luau scripts for downstream processing.
Two key use cases:

1. **Data dump for offline analysis** — LLM fetches large datasets via API, writes them to
   disk so the user or another tool (Python, etc.) can analyze them later.
2. **Multi-stage LLM pipelines** — Luau script writes intermediate data, then the LLM
   invokes a separate tool (Python, shell) to process it, all within one conversation.

Currently the sandbox has zero filesystem access. Script results are returned inline,
which is fine for small payloads but impractical for large datasets.

## Design

### API

A single Luau global: `file.save(filename, content)`

- `filename`: string — relative path within the output directory (subdirs allowed)
- `content`: string — data to write (LLM formats using `json.encode()`, `string.format()`, etc.)
- Returns: absolute path string on success
- Raises: Lua error on failure (bad filename, size limit exceeded, I/O error)

The existing sandbox already provides `json.encode()`, `string.format()`, `string.rep()`,
`table.concat()`, and pattern matching — so the LLM can construct CSV, JSON, TSV, XML, or
any custom format as a string before saving.

### Configuration

Added to TOML config under `[output]`:

```toml
[output]
dir = "./code-mcp-output"       # optional, defaults to ./code-mcp-output
max_bytes = 52428800             # optional, defaults to 50MB per script execution
enabled = true                   # optional, defaults to true
```

CLI flag: `--output-dir <path>` on the `serve` command (overrides config).

The output directory is created on first write, not at startup.

### Implementation Location

Registered in `sandbox.rs` as a global table, following the same pattern as `json` and
`print`. The Rust closure backing the function handles file I/O, path validation, and
size tracking.

### Safety

**Filename validation:**
- Reject absolute paths
- Reject path traversal (`..` components)
- Reject null bytes
- Allow subdirectories within output dir (`data/results.csv` is valid)
- Auto-create subdirectories as needed

**Size enforcement:**
- Track cumulative bytes written per script execution
- Error if a write would exceed `max_bytes`
- Script receives a Lua error it can handle

**Overwrite behavior:**
- Overwrites existing files (same script re-running produces fresh output)

### Execution Result

The script execution response gains a `files_written` field:

```json
{
  "result": { "rows": 10000 },
  "logs": ["Processing complete"],
  "stats": { "api_calls": 3, "duration_ms": 1200 },
  "files_written": [
    { "name": "dataset.csv", "path": "/home/user/code-mcp-output/dataset.csv", "bytes": 524288 }
  ]
}
```

This ensures the LLM always knows what was written and where, so it can:
- Tell the user where to find files
- Pass paths to downstream tools (Python, etc.)

### Hosted Mode

`file.save()` is local-only. In hosted/remote mode (detected via `per_request_auth`),
the `file` table is not registered in the sandbox. Scripts that attempt to call
`file.save()` get a standard nil error.

### Example Usage

```lua
-- Fetch large dataset and save as CSV for Python analysis
local data = sdk.list_transactions({ limit = 10000 })

local csv = "id,date,amount,category\n"
for _, tx in ipairs(data.items) do
    csv = csv .. string.format("%s,%s,%.2f,%s\n", tx.id, tx.date, tx.amount, tx.category)
end

file.save("transactions.csv", csv)

-- Also save a JSON summary
local summary = {
    total = #data.items,
    sum = 0,
}
for _, tx in ipairs(data.items) do
    summary.sum = summary.sum + tx.amount
end
file.save("summary.json", json.encode(summary))

return summary
```

## Testing Strategy

- **Unit tests:** Filename validation (traversal, absolute paths, null bytes, valid subdirs)
- **Integration test:** Config with output enabled, execute script with `file.save()`,
  verify file on disk, verify `files_written` in result
- **E2e test:** Python e2e test exercising the MCP tool and checking the output directory
- **Negative tests:** Hosted mode (function absent), size limit exceeded, disabled via config

## Decisions Summary

| Aspect | Decision |
|--------|----------|
| API | `file.save(filename, content)` |
| Location | `sandbox.rs` global table |
| Config | `[output]` section: `dir`, `max_bytes`, `enabled` |
| CLI | `--output-dir <path>` on `serve` |
| Default dir | `./code-mcp-output/` |
| Size limit | 50MB per script execution, configurable |
| Safety | No traversal, no absolute paths, no null bytes |
| Overwrite | Yes |
| Return | Absolute path on success, Lua error on failure |
| Result | `files_written` array in execution response |
| Hosted mode | Disabled — `file` table not registered |
| Read access | Not included (future enhancement) |
| Default state | Enabled by default |
