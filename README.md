# code-mcp

Turn OpenAPI specs into scriptable MCP servers. One round-trip instead of many.

## The Problem

AI agents using MCP tools over complex APIs waste resources. Each API call becomes a separate tool invocation, and the LLM round-trips all intermediate state even when no judgment is needed. The LLM becomes an expensive data shuttle.

## The Solution

code-mcp gives the LLM a [Luau](https://luau-lang.org/) scripting runtime with an auto-generated, strongly-typed SDK derived from OpenAPI specs. The LLM writes a script that chains multiple API calls, sends it for execution, and gets back the result. One round-trip instead of many.

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

## How It Works

1. The agent connects to the MCP server.
2. It explores the SDK using documentation tools (`list_apis`, `list_functions`, `get_function_docs`, `search_docs`, `get_schema`) or by browsing resources (`sdk://petstore/overview`, `sdk://petstore/functions`, etc.).
3. It writes a Luau script that chains SDK calls.
4. It sends the script to `execute_script`.
5. It gets back the result, captured logs, and execution stats in a single response.

Example script the LLM might write:

```lua
-- Get all pets, then fetch details for the first one
local pets = sdk.list_pets({ limit = 5 })
local first = pets[1]
local details = sdk.get_pet({ pet_id = first.id })
return { pet = details, total = #pets }
```

The response includes the return value as JSON, any `print()` output captured as logs, and stats (API call count, wall-clock duration).

## CLI Reference

### `code-mcp run`

Generate and serve in one step. This is the most common subcommand.

```
code-mcp run <SPECS>... [OPTIONS]
```

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

If no specs and no `--config` are provided, `code-mcp run` looks for `code-mcp.toml` in the current directory.

### `code-mcp generate`

Code generation only. Produces a manifest and SDK annotations without starting a server.

```
code-mcp generate <SPECS>... [-o <DIR>] [--config <FILE>]
```

Output directory defaults to `./output`. Generates `manifest.json` and `sdk/*.luau`. Use `--config` to load specs from a TOML config file instead of positional arguments.

### `code-mcp serve`

Start an MCP server from a pre-generated output directory.

```
code-mcp serve <DIR> [OPTIONS]
```

Accepts the same options as `run` (`--auth`, `--transport`, `--port`, `--timeout`, `--memory-limit`, `--max-api-calls`, `--auth-authority`, `--auth-audience`, `--auth-jwks-uri`).

## Authentication

There are two separate authentication layers.

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

### MCP-Layer Authentication

This controls who can connect to the code-mcp server itself. It only applies when using HTTP/SSE transport.

- JWT validation with OIDC discovery
- Enable with `--auth-authority` and `--auth-audience`
- Optionally override the JWKS endpoint with `--auth-jwks-uri`
- Publishes `/.well-known/oauth-protected-resource` for client discovery

For local stdio usage, this layer is not needed -- the MCP client and server share the same trust boundary.

## Frozen Parameters

Frozen parameters are server-side fixed values that are injected into API calls at request time. They are completely hidden from the LLM — stripped from tool schemas, documentation, and search results. Use them to hardcode values like API versions, tenant IDs, or environment-specific settings.

Configure frozen params in `code-mcp.toml` at two levels:

```toml
# Global — applies to every API
[frozen_params]
api_version = "v2"

# Per-API — applies only to this API's operations
[apis.petstore]
spec = "petstore.yaml"
[apis.petstore.frozen_params]
tenant_id = "abc-123"
```

**Precedence:** Per-API values override global values when the same parameter name appears in both. Non-matching parameter names (params that don't exist in an operation) are silently ignored.

**How it works:** During code generation, frozen parameters retain their full metadata (name, location, type) but are marked with a fixed value. At runtime, the server injects the configured value into the correct location (path, query string, or header) without the LLM needing to know about them.

## Execution Limits

| Flag              | Default | Controls                                    |
| ----------------- | ------- | ------------------------------------------- |
| `--timeout`       | 30s     | Wall-clock deadline per script execution    |
| `--memory-limit`  | 64 MB   | Maximum Luau VM memory allocation           |
| `--max-api-calls` | 100     | Maximum upstream HTTP requests per script   |

CPU is limited indirectly by the wall-clock timeout. There is no separate instruction-count limit.

## MCP Tools and Resources

### Tools

| Tool                | Description                                                          |
| ------------------- | -------------------------------------------------------------------- |
| `list_apis`         | List loaded APIs with names, descriptions, base URLs, endpoint counts |
| `list_functions`    | List SDK functions, filterable by API or tag                         |
| `get_function_docs` | Full Luau type annotation for a function                             |
| `search_docs`       | Full-text search across all SDK documentation                        |
| `get_schema`        | Full Luau type annotation for a schema/type                          |
| `execute_script`    | Execute a Luau script against the SDK                                |

### Resources

Browsable SDK documentation, accessible via `resources/read`:

| URI pattern                      | Content                    |
| -------------------------------- | -------------------------- |
| `sdk://{api}/overview`           | API overview               |
| `sdk://{api}/functions`          | All function signatures    |
| `sdk://{api}/schemas`            | All type definitions       |
| `sdk://{api}/functions/{name}`   | Individual function docs   |
| `sdk://{api}/schemas/{name}`     | Individual schema docs     |

## Sandbox Security

Scripts execute in a sandboxed Luau VM. Here is what is and is not available.

**Allowed:**

- Standard libraries: `string`, `table`, `math`
- `os.clock()` (wall-clock timing only)
- `print()` (captured to logs, not written to stdout)
- `json.encode()` / `json.decode()`
- `sdk.*` functions (generated from the OpenAPI spec)

**Blocked:**

- `io` (file I/O)
- `os.execute` (shell access)
- `loadfile`, `dofile`, `require` (module loading)
- `debug` library
- `string.dump` (bytecode access)
- `load` (dynamic code loading)
- Raw network access
- Filesystem access

**Enforcement mechanisms:**

- Luau native sandbox mode (read-only globals, isolated per-script environments)
- Configurable memory limit
- Wall-clock timeout via Luau interrupt callbacks
- API call counter per execution
- Fresh VM per execution (no state leaks between scripts)
- Credentials never exposed to Luau -- injected server-side

**A note on hosting.** If you deploy code-mcp over HTTP for multiple users, you are offering your compute as a code sandbox. The sandboxing limits the abuse surface, but you should deploy behind appropriate resource constraints and network policies. For most use cases, running locally over stdio with your own credentials is the simplest and most secure option.

## Docker

Build and run:

```bash
docker build -t code-mcp .
docker run code-mcp https://api.example.com/openapi.json
```

For HTTP transport:

```bash
docker run -p 8080:8080 code-mcp \
  https://api.example.com/openapi.json \
  --transport sse --port 8080
```

## Building from Source

```bash
git clone https://github.com/alenna/code-mcp.git
cd code-mcp
cargo build --release
cargo test
```

Requires Rust 1.85+ (uses edition 2024).

## License

MIT
