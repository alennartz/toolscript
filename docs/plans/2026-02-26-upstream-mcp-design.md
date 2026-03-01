# Upstream MCP Server Support Design

## Summary

Add support for upstream MCP servers so that Luau scripts can call tools from external MCP servers alongside OpenAPI-derived functions. Both live under the unified `sdk.*` namespace. ToolScript connects to upstream servers as an MCP client, discovers their tools, and registers them as callable Luau closures.

## Goals

- Luau scripts can mix OpenAPI calls and MCP tool calls in a single execution
- MCP tools are fully discoverable through `list_functions`, `search_docs`, `get_function_docs`
- Unified `sdk.<source>.<function>()` namespace for both OpenAPI and MCP
- MCP-only mode (no OpenAPI specs required)
- Support stdio, SSE, and Streamable HTTP transports for upstream connections

## Non-Goals

- Re-exposing upstream MCP tools in ToolScript's own MCP tool list (tools are only callable from Luau)
- MCP resource or prompt proxying (tools only)
- Static codegen for MCP tools (`generate` command unchanged)

## Data Model

New types in `manifest.rs`:

```rust
pub struct McpServerEntry {
    pub name: String,
    pub description: Option<String>,
    pub tools: Vec<McpToolDef>,
}

pub struct McpToolDef {
    pub name: String,
    pub server: String,
    pub description: Option<String>,
    pub params: Vec<McpParamDef>,
    pub schemas: Vec<SchemaDef>,       // from $defs/definitions in input_schema
    pub output_schemas: Vec<SchemaDef>, // from output_schema if present
}

pub struct McpParamDef {
    pub name: String,
    pub luau_type: String,
    pub required: bool,
    pub description: Option<String>,
}
```

`Manifest` gains a new field:

```rust
pub struct Manifest {
    pub apis: Vec<ApiConfig>,
    pub functions: Vec<FunctionDef>,
    pub schemas: Vec<SchemaDef>,
    pub mcp_servers: Vec<McpServerEntry>,
}
```

## Configuration

### TOML (`toolscript.toml`)

```toml
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
env = { HOME = "/tmp" }  # optional env vars

[mcp_servers.legacy]
url = "https://mcp.example.com/sse"
transport = "sse"

[mcp_servers.remote]
url = "https://mcp.example.com/mcp"
# transport defaults to "streamable-http"
```

### CLI

```
toolscript run [SPECS]... --mcp <name>=<command_or_url>
```

- Value starts with `http://` or `https://` → url, transport defaults to `streamable-http`
- Otherwise → split on spaces, first token is `command`, rest is `args`, transport is stdio

### Transport rules

- `command` present → stdio (inferred, no `transport` field needed)
- `url` present → `transport` field available, defaults to `"streamable-http"`, can be `"sse"`
- Must have exactly one of `command` or `url`
- `transport` field with `command` → validation error
- `args`/`env` with `url` → validation error
- CLI `--mcp` overrides TOML entries with the same name

### MCP-only mode

If no specs are provided and no `[apis]` in config, but `[mcp_servers]` exists, ToolScript starts with only MCP tools. Zero APIs plus zero MCP servers → startup error.

## MCP Client Lifecycle

### Startup

1. Parse config / CLI `--mcp` flags
2. For each MCP server entry, connect:
   - **stdio:** Spawn child process via `TokioChildProcess`, `().serve(transport).await`
   - **streamable-http / sse:** Create HTTP client transport to `url`, `().serve(transport).await`
3. Call `list_all_tools()` on each connected server
4. Convert each `Tool` → `McpToolDef` (JSON Schema → `Vec<McpParamDef>`, extract `$defs`/`definitions` → `Vec<SchemaDef>`)
5. Populate `manifest.mcp_servers`
6. Hold `RunningService<RoleClient, ()>` handles in `McpClientMap` (name → client)

### Reconnection

On `call_tool` transport failure:
- Attempt one reconnect using original config (respawn process / re-establish HTTP)
- If reconnect succeeds → retry the `call_tool` once
- If reconnect fails → raise Lua error
- Log reconnection events at info level

### Shutdown

Call `service.close()` on each client. For stdio, this terminates the child process.

### Error handling

If an upstream server fails to connect at startup, log warning and continue with remaining servers.

## Luau Registration

New `register_mcp_tools` function in `registry.rs`. For each `McpServerEntry`:

1. Create sub-table `sdk.<server_name>` in the Lua `sdk` global
2. For each tool, create a Lua closure that:
   - Extracts params table from Lua arg
   - Converts to `serde_json::Map` (the `arguments` for `CallToolRequestParams`)
   - Calls `service.call_tool()` via `block_in_place`
   - Increments shared `api_call_counter` (same limit as HTTP calls)
   - Returns result to Lua

### Return value mapping

- `structured_content` present → return as Lua table
- Single `Content::Text` → return as Lua string
- `is_error` → raise Lua error
- Multiple content items → return as Lua array

No magic JSON parsing of text content. Script author uses `json.decode()` if needed.

### Calling convention

```lua
sdk.filesystem.read_file({ path = "/tmp/data.txt" })
sdk.database.query({ sql = "SELECT * FROM users", limit = 10 })
```

All MCP tools use the table-based params convention. If a tool has no input params, it can be called with no arguments.

## Discovery Integration

MCP tools appear in all documentation tools:

- **`list_functions`**: MCP tools listed with `server_name.tool_name` format. Filterable by server name (same as API name filter).
- **`search_docs`**: MCP tool names, descriptions, and param names indexed alongside OpenAPI functions and schemas.
- **`get_function_docs`**: Returns Luau annotation plus all transitively referenced type definitions in one response. Works identically for OpenAPI and MCP functions.
- **MCP resources**: `sdk://<server>/overview` and `sdk://<server>/functions` added for each MCP server.

## `get_schema` Removal

`get_schema` is removed as a tool. Its functionality is absorbed into `get_function_docs`, which now returns the function signature plus all transitively referenced type definitions.

For OpenAPI: `$ref` to `#/components/schemas/X` are resolved transitively. All referenced types appended as `export type` blocks.

For MCP: `$ref` to `#/$defs/X` (or `#/definitions/X`) are resolved transitively. Same treatment.

Example `get_function_docs` output:

```luau
-- Create a new pet
-- @param body - The pet to create
function sdk.petstore.create_pet(body: NewPet): Pet end

export type NewPet = {
    name: string,
    tag: Tag?,
}

export type Pet = {
    id: number,
    name: string,
    owner: User?,
    tag: Tag?,
}

export type User = {
    id: number,
    name: string,
    email: string,
}

export type Tag = {
    id: number,
    name: string,
}
```

`search_docs` still indexes schema names and fields for discoverability.

## Shared Annotation Utilities

New module `src/codegen/luau_types.rs`:

- `json_schema_type_to_luau()` — maps JSON Schema type strings to Luau type strings
- `json_schema_to_params()` — converts JSON Schema `properties` + `required` to `Vec<McpParamDef>`
- `extract_schema_defs()` — extracts `$defs`/`definitions` from JSON Schema into `Vec<SchemaDef>`
- `collect_referenced_schemas()` — walks ref chain transitively, returns deduplicated `Vec<SchemaDef>`

Existing `param_type_to_luau` and `field_type_to_luau` in `annotations.rs` refactored to call through shared utilities.

| JSON Schema | Luau |
|---|---|
| `"string"` | `string` |
| `"integer"`, `"number"` | `number` |
| `"boolean"` | `boolean` |
| `"array"` + items | `{ItemType}` |
| `"object"` + properties (inline, no `$ref`) | `{ field1: type1, field2: type2 }` |
| `"object"` via `$ref` | Named type reference (e.g. `Pet`) |
| `"object"` + `additionalProperties` | `{ [string]: ValueType }` |
| enum | `("val1" \| "val2")` |
| anything else / missing | `any` |

## Inline Object Bug Fix

Pre-existing bug: inline nested objects in OpenAPI schemas render as `"unknown"` instead of proper Luau types. No test coverage exists for this case.

### Fix

Extend `FieldType` with a new variant:

```rust
pub enum FieldType {
    String,
    Integer,
    Number,
    Boolean,
    Array { items: Box<Self> },
    Object { schema: String },
    InlineObject { fields: Vec<FieldDef> },  // NEW
    Map { value: Box<Self> },
}
```

**Parser**: `schema_kind_to_field_type` for `Type::Object` with properties recursively extracts fields into `InlineObject` instead of returning `Object { schema: "unknown" }`.

**Annotations**: `field_type_to_luau` renders `InlineObject` as nested inline Luau types: `{ field1: type1, field2: type2? }`.

**Request body**: Inline request/response body schemas generate `InlineObject` or synthesize a named schema rather than emitting `"unknown"`.

Applies to both OpenAPI and MCP paths since both go through the same type conversion.

## Cargo.toml Changes

```toml
rmcp = { version = "0.16", features = [
    "server",
    "client",
    "transport-io",
    "transport-child-process",
    "transport-streamable-http-server",
    "transport-streamable-http-client-reqwest",
] }
```

No new crates needed.

## CLI Changes

`--mcp` flag added to `run` and `serve` commands. Not added to `generate` (MCP tools are dynamic).

```bash
# OpenAPI + MCP
toolscript run petstore=spec.json --mcp filesystem='npx -y @modelcontextprotocol/server-filesystem /tmp'

# MCP only
toolscript run --mcp filesystem='npx -y @modelcontextprotocol/server-filesystem /tmp'

# Remote
toolscript run --mcp remote=https://mcp.example.com/mcp
```

## Testing Strategy

### Unit tests
- `luau_types.rs` — all type variants, inline nested objects, `$ref` resolution, `$defs` extraction
- `annotations.rs` — function annotations with transitive schema appendage, both OpenAPI and MCP
- `config.rs` — TOML `[mcp_servers]` parsing, CLI `--mcp` parsing, validation rules
- `manifest.rs` — `McpServerEntry`/`McpToolDef` serialization
- `registry.rs` — MCP tool Lua closure registration with mock client

### Integration tests
- Config → connect → discover → execute script calling MCP tool
- MCP-only mode
- Mixed OpenAPI + MCP with unified namespace
- Reconnection after upstream server failure

### Inline object bug fix tests
- OpenAPI schema with inline nested object → correct Luau type (not `"unknown"`)
- Deeply nested inline objects
- Mixed `$ref` and inline in same schema

### Mock MCP server
In-process MCP server using rmcp server-side for tests. Registers dummy tools with known schemas including `$ref`s and nested objects.
