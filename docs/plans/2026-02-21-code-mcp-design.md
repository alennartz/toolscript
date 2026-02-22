# code-mcp Design

## Overview

code-mcp is a tool that takes OpenAPI specifications and generates an MCP server with a Lua scripting runtime. Instead of exposing individual API endpoints as separate MCP tools, it gives the LLM an SDK and a script executor — allowing it to chain multiple API calls in a single round-trip.

## Approach

**Lua 5.4 + Unified Rust CLI.** One Rust binary handles both code generation and serving. Lua 5.4 via mlua for the scripting runtime. LuaLS-compatible type annotations for SDK documentation. Rust-side request validation at the boundary.

## System Architecture

The CLI has three modes:

- **`code-mcp run <spec-source...>`** — Generate then serve in one step. The default happy path.
- **`code-mcp generate <spec-source...> -o ./output/`** — Generate manifest and SDK annotations only, for inspection or customization.
- **`code-mcp serve ./output/`** — Start the MCP server from a pre-generated directory.

Spec sources can be local file paths or URLs.

```
code-mcp run <spec-source...>
  1. Fetch/read spec(s)
  2. Generate manifest + annotations to temp dir
  3. Start MCP server from that dir
```

## Codegen Pipeline

### Input

One or more OpenAPI 3.0/3.1 specifications (JSON or YAML, local file or URL).

### Output

```
output/
├── manifest.json          # Machine-readable endpoint registry
└── sdk/
    ├── _meta.lua          # SDK metadata, version, base URLs
    ├── users.lua          # Type annotations grouped by tag/path
    ├── orders.lua
    └── ...
```

### Manifest Format

The manifest is what the Rust runtime reads to register functions:

```json
{
  "apis": [
    {
      "name": "petstore",
      "base_url": "https://petstore.example.com/v1",
      "auth": {
        "type": "bearer",
        "header": "Authorization",
        "prefix": "Bearer "
      }
    }
  ],
  "functions": [
    {
      "name": "get_pet",
      "api": "petstore",
      "method": "GET",
      "path": "/pets/{pet_id}",
      "parameters": [
        { "name": "pet_id", "in": "path", "type": "string", "required": true }
      ],
      "response_schema": { "$ref": "#/schemas/Pet" }
    }
  ],
  "schemas": {
    "Pet": {
      "type": "object",
      "properties": {
        "id": { "type": "string" },
        "name": { "type": "string" },
        "status": { "type": "string", "enum": ["available", "pending", "sold"] }
      }
    }
  }
}
```

### Lua Annotation Files

Generated with full documentation extracted from the OpenAPI spec:

```lua
-- sdk/pets.lua
-- Petstore API v1.2.3
-- A sample API that uses a petstore as an example
-- Docs: https://petstore.example.com/docs

--- Get a pet by ID
---
--- Returns a single pet by its unique identifier. Returns 404 if the
--- pet does not exist. The pet object includes its current adoption status.
---
--- @param pet_id string The pet's unique identifier (UUID format)
--- @return Pet A pet object
---
--- Example:
---   local pet = sdk.get_pet("abc-123")
---   -- pet.name => "Fido"
---   -- pet.status => "available"
function sdk.get_pet(pet_id) end

--- @class Pet
--- @field id string The pet's unique identifier (UUID format)
--- @field name string The pet's display name
--- @field status "available"|"pending"|"sold" Current adoption status
--- @field tag? string Optional classification tag
```

### Documentation Extraction

Every human-facing field in the OpenAPI spec flows into the generated output:

| OpenAPI field | Generated output |
|---|---|
| `info.title`, `info.description`, `info.version` | File header comment + API metadata |
| `operation.summary` | First line of function doc |
| `operation.description` | Extended function doc block |
| `operation.deprecated` | `@deprecated` annotation |
| `parameter.description` | `@param` description |
| `parameter.example` | Example in doc block |
| `schema.description` | `@field` description on classes |
| `schema.default` | Noted in param description |
| `schema.enum` | Literal union types in annotations |
| `response.description` | `@return` description |
| `tag.description` | Module-level doc comment |
| `externalDocs.url` | Link in header/doc block |

### Naming Strategy

- Function names derived from `operationId` if present, else `method_path`
- Functions grouped into files by OpenAPI tag, or by first path segment if untagged
- Collision handling: append discriminating path segments

### OpenAPI Subset (v1)

**Supported:** Path, query, and header parameters. JSON request/response bodies. `$ref` resolution (local refs). Basic types: string, integer, number, boolean, array, object. Required/optional parameters. Enum values. Common auth: Bearer, API key (header/query), Basic.

**Deferred:** `oneOf`/`anyOf`/`allOf` polymorphism. File upload/download. Webhooks/callbacks. Cookie parameters. XML content types.

## MCP Interface

### Documentation Exploration Tools

- **`list_apis`** — List all loaded APIs with names, descriptions, base URLs, endpoint counts.
- **`list_functions`** — List available SDK functions, optionally filtered by API or tag.
- **`get_function_docs`** — Full documentation for a specific function: parameters, return types, examples, descriptions.
- **`search_docs`** — Full-text search across all SDK documentation.
- **`get_schema`** — Full definition of a data type (class/object) with all fields.

### Script Execution Tool

**`execute_script`** — Run a Lua script against the SDK.

```json
{
  "script": "local pets = sdk.list_pets('available', 5)\nreturn pets",
  "timeout_ms": 30000
}
```

Authentication for upstream APIs is provided via `_meta.auth` on the MCP protocol request (see `docs/plans/2026-02-22-http-auth-design.md`). The `_meta` field is injected by the MCP client at the transport layer and is invisible to the LLM. Fallback: credentials can come from environment variables on the server process.

### MCP Resources

Browsable documentation exposed as MCP resources:

- `sdk://petstore/overview` — API overview and description
- `sdk://petstore/functions` — All function signatures
- `sdk://petstore/schemas` — All type definitions
- `sdk://petstore/functions/get_pet` — Individual function docs
- `sdk://petstore/schemas/Pet` — Individual schema docs

## Runtime & Sandbox

### Sandbox Constraints

**Allowed:** `string`, `table`, `math`, `os.clock` (timing only). Generated SDK functions. `print()` (captured to logs). `json.encode()` / `json.decode()` (Rust-backed).

**Blocked:** `io`, `os.execute`, `os.getenv`, `os.remove`, `loadfile`, `dofile`, `require`, `debug` library. No raw network access. No filesystem access.

### Execution Flow

1. Rust receives script via `execute_script`
2. Parse and validate Lua syntax
3. Create sandbox: load SDK functions with auth config from server-side
4. Execute script with timeout watchdog
5. SDK function calls → Rust makes HTTP request (injects auth, sends, parses JSON, returns Lua table)
6. Script returns final value
7. Rust serializes result to JSON
8. MCP response: `{ result, logs, stats }`

### Resource Limits

- Execution timeout: configurable, default 30s
- Memory limit: configurable Lua memory cap
- API call limit: max HTTP requests per script execution

### Error Handling

```json
{
  "error": {
    "phase": "execution",
    "message": "attempt to index a nil value",
    "line": 5,
    "function": "get_pet",
    "api_status": 404
  }
}
```

Phases: `parse`, `execution`, `api_call`, `timeout`, `validation`.

## Security Model

- **No raw HTTP in Lua.** Each SDK function is a Rust-registered function bound to one endpoint. No generic escape hatch.
- **Credentials never in Lua.** Auth is server-side config (env vars) or MCP client-injected parameter. Lua sandbox has no access.
- **SDK-only sandbox.** Scripts can only use the generated SDK and basic Lua libs.
- **Manifest-driven registration.** The runtime only exposes endpoints defined in the manifest.

## Distribution

### Binary

```bash
cargo install code-mcp
code-mcp run https://api.example.com/openapi.json
```

### Container

```bash
docker run code-mcp https://api.example.com/openapi.json
docker run -p 8080:8080 code-mcp --transport sse --port 8080 https://api.example.com/openapi.json
docker run -e API_KEY=sk-... code-mcp https://api.example.com/openapi.json
```

### MCP Client Configuration

```json
{
  "mcpServers": {
    "petstore": {
      "command": "code-mcp",
      "args": ["run", "https://petstore.example.com/openapi.json"],
      "env": { "PETSTORE_API_KEY": "sk-..." }
    }
  }
}
```

## Tech Stack

- **Rust** — CLI, MCP server, HTTP client, codegen, Lua hosting
- **Lua 5.4 via mlua** — Scripting runtime
- **openapiv3** — OpenAPI spec parsing
- **serde / serde_json / serde_yaml** — Serialization
- **reqwest** — HTTP client for API calls
- **tokio** — Async runtime
- **clap** — CLI argument parsing
