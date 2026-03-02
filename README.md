# toolscript

Turn OpenAPI specs and MCP servers into a scriptable runtime. One round-trip instead of many.

## The Problem

AI agents using MCP tools over complex APIs waste resources. Each API call becomes a separate tool invocation, and the LLM round-trips all intermediate state even when no judgment is needed. The LLM becomes an expensive data shuttle.

## The Solution

toolscript gives the LLM a [Luau](https://luau-lang.org/) scripting runtime with a strongly-typed SDK. The SDK can be auto-generated from OpenAPI specs, populated from upstream MCP servers, or both. The LLM writes a script that chains multiple calls, sends it for execution, and gets back the result. One round-trip instead of many.

## Quick Start

```bash
cargo install --path .
```

Point at an OpenAPI spec and provide your API key:

```bash
export MY_TOKEN=your-token-here
toolscript run petstore=https://petstore3.swagger.io/api/v3/openapi.json \
  --auth petstore:MY_TOKEN
```

Or use a config file (`toolscript.toml`):

```toml
[apis.petstore]
spec = "https://petstore3.swagger.io/api/v3/openapi.json"
auth = "your-token-here"
```

```bash
toolscript run
```

Or connect to upstream MCP servers instead of (or alongside) OpenAPI specs:

```bash
# MCP-only
toolscript run --mcp filesystem='npx -y @modelcontextprotocol/server-filesystem /tmp'

# Mixed: OpenAPI + MCP
toolscript run petstore=petstore.yaml \
  --auth petstore:MY_TOKEN \
  --mcp filesystem='npx -y @modelcontextprotocol/server-filesystem /tmp'
```

Add the server to your MCP client config:

```json
{
  "mcpServers": {
    "petstore": {
      "command": "toolscript",
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
2. It explores the SDK using documentation tools (`list_apis`, `list_functions`, `get_function_docs`, `search_docs`) or by browsing resources (`sdk://petstore/overview`, `sdk://petstore/functions`, etc.).
3. It writes a Luau script that chains SDK calls.
4. It sends the script to `execute_script`.
5. It gets back the result and captured logs in a single response.

Example script the LLM might write:

```lua
-- OpenAPI calls
local pets = sdk.list_pets({ limit = 5 })
local first = pets[1]
local details = sdk.get_pet({ pet_id = first.id })
return { pet = details, total = #pets }
```

Scripts can also call upstream MCP tools in the same namespace:

```lua
-- MCP tool call (filesystem server)
local content = sdk.filesystem.read_file({ path = "/tmp/data.txt" })
return json.decode(content)
```

Both OpenAPI functions and MCP tools coexist under `sdk.*` and can be mixed freely in a single script. The response includes the return value as JSON, any `print()` output captured as logs, and a `files_touched` array summarizing files written or removed via the sandboxed `io` library.

## CLI Reference

### `toolscript run`

Generate and serve in one step. This is the most common subcommand.

```
toolscript run [SPECS]... [OPTIONS]
```

| Flag               | Default | Description                                    |
| ------------------ | ------- | ---------------------------------------------- |
| `--config`         | --      | Path to TOML config file                       |
| `--auth`           | --      | API auth: `name:ENV_VAR` or `ENV_VAR`          |
| `--mcp`            | --      | Upstream MCP server: `name=command` or `name=url` |
| `--transport`      | `stdio` | Transport type (`stdio`, `sse`)                |
| `--port`           | `8080`  | Port for HTTP/SSE transport                    |
| `--timeout`        | `30`    | Script execution timeout (seconds)             |
| `--memory-limit`   | `64`    | Luau VM memory limit (MB)                      |
| `--max-api-calls`  | `100`   | Max upstream calls per script (API + MCP)      |
| `--io-dir`         | --      | I/O directory for sandboxed file access         |
| `--auth-authority` | --      | OAuth issuer URL (enables JWT auth)            |
| `--auth-audience`  | --      | Expected JWT audience                          |
| `--auth-jwks-uri`  | --      | Explicit JWKS URI override                     |

Specs are optional when `--mcp` or `[mcp_servers]` config provides at least one source. If no specs, no `--mcp`, and no `--config` are provided, `toolscript run` looks for `toolscript.toml` in the current directory.

### `toolscript generate`

Code generation only. Produces a manifest and SDK annotations without starting a server.

```
toolscript generate <SPECS>... [-o <DIR>] [--config <FILE>]
```

Output directory defaults to `./output`. Generates `manifest.json` and `sdk/*.luau`. Use `--config` to load specs from a TOML config file instead of positional arguments.

### `toolscript serve`

Start an MCP server from a pre-generated output directory.

```
toolscript serve <DIR> [OPTIONS]
```

Accepts the same options as `run` (`--auth`, `--mcp`, `--transport`, `--port`, `--timeout`, `--memory-limit`, `--max-api-calls`, `--io-dir`, `--auth-authority`, `--auth-audience`, `--auth-jwks-uri`).

## Authentication

There are two separate authentication layers.

### Upstream API Credentials

These are the credentials toolscript uses to call the APIs behind the SDK.

**CLI `--auth` flag** (quick start):

```bash
# Named: --auth name:ENV_VAR
toolscript run petstore=spec.yaml --auth petstore:MY_TOKEN

# Unnamed (single-spec only): --auth ENV_VAR
toolscript run spec.yaml --auth MY_TOKEN
```

The tool reads the value of the environment variable at startup. The secret never appears in the command itself.

**Config file** (`toolscript.toml`):

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
toolscript run --config toolscript.toml
# Or just have toolscript.toml in the current directory:
toolscript run
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

This controls who can connect to the toolscript server itself. It only applies when using HTTP/SSE transport.

- JWT validation with OIDC discovery
- Enable with `--auth-authority` and `--auth-audience`
- Optionally override the JWKS endpoint with `--auth-jwks-uri`
- Publishes `/.well-known/oauth-protected-resource` for client discovery

For local stdio usage, this layer is not needed -- the MCP client and server share the same trust boundary.

## Frozen Parameters

Frozen parameters are server-side fixed values that are injected into API calls at request time. They are completely hidden from the LLM — stripped from tool schemas, documentation, and search results. Use them to hardcode values like API versions, tenant IDs, or environment-specific settings.

Configure frozen params in `toolscript.toml` at two levels:

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

## Upstream MCP Servers

toolscript can connect to external MCP servers and expose their tools as callable Luau functions alongside OpenAPI-generated functions. Tools from upstream MCP servers appear in the `sdk.<server>.<tool>()` namespace.

### CLI `--mcp` flag

```bash
# Stdio: name=command with args
toolscript run --mcp filesystem='npx -y @modelcontextprotocol/server-filesystem /tmp'

# HTTP: name=url (uses streamable-http transport)
toolscript run --mcp remote=https://mcp.example.com/mcp

# Multiple servers
toolscript run --mcp filesystem='npx -y @modelcontextprotocol/server-filesystem /tmp' \
               --mcp db='npx -y @modelcontextprotocol/server-sqlite db.sqlite'
```

If the value starts with `http://` or `https://`, it's treated as a URL. Otherwise, it's split on spaces: the first token is the command, the rest are arguments.

### Config file

```toml
# Stdio-based (spawns a child process)
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
env = { HOME = "/tmp" }

# HTTP-based (streamable-http transport)
[mcp_servers.remote]
url = "https://mcp.example.com/mcp"
```

Each entry must have exactly one of `command` or `url`. The `args` and `env` fields are only valid with `command`. URL-based servers use the streamable-http transport. Legacy SSE transport is not supported.

CLI `--mcp` flags are merged with config file entries. If both define the same server name, the CLI flag wins.

### How MCP tools appear

MCP tools are fully integrated into the discovery tools and resources. `list_apis` includes MCP servers alongside OpenAPI APIs. `list_functions` returns MCP tools alongside OpenAPI functions, filterable by server name. `get_function_docs` returns the full Luau type annotation for any MCP tool. `search_docs` searches across MCP tool names, descriptions, and parameters.

In Luau scripts, MCP tools are namespaced under the server name:

```lua
-- Call an MCP tool
local result = sdk.filesystem.read_file({ path = "/tmp/data.txt" })

-- MCP tool results: text content is returned as a string,
-- structured content as a table. Parse JSON yourself if needed:
local data = json.decode(result)
```

## Execution Limits

| Flag              | Default | Controls                                    |
| ----------------- | ------- | ------------------------------------------- |
| `--timeout`       | 30s     | Wall-clock deadline per script execution    |
| `--memory-limit`  | 64 MB   | Maximum Luau VM memory allocation           |
| `--max-api-calls` | 100     | Maximum upstream calls per script (API + MCP) |

Both OpenAPI HTTP requests and MCP tool calls count toward the same limit. CPU is limited indirectly by the wall-clock timeout. There is no separate instruction-count limit.

## MCP Tools and Resources

### Tools

| Tool                | Description                                                                  |
| ------------------- | ---------------------------------------------------------------------------- |
| `list_apis`         | List loaded APIs and MCP servers with names, descriptions, and counts        |
| `list_functions`    | List SDK functions and MCP tools, filterable by API/server or tag            |
| `get_function_docs` | Full Luau type annotation for a function or MCP tool, with referenced schemas |
| `search_docs`       | Full-text search across all SDK and MCP tool documentation                   |
| `execute_script`    | Execute a Luau script against the SDK                                        |

### Resources

Browsable SDK documentation, accessible via `resources/read`:

| URI pattern                      | Content                                  |
| -------------------------------- | ---------------------------------------- |
| `sdk://{api}/overview`           | API or MCP server overview               |
| `sdk://{api}/functions`          | All function/tool signatures             |
| `sdk://{api}/schemas`            | All type definitions (OpenAPI only)       |
| `sdk://{api}/functions/{name}`   | Individual function docs (OpenAPI only)   |
| `sdk://{api}/schemas/{name}`     | Individual schema docs (OpenAPI only)     |

The `overview` and `functions` resources are generated for both OpenAPI APIs and upstream MCP servers.

## Sandbox Security

Scripts execute in a sandboxed Luau VM. Here is what is and is not available.

**Allowed:**

- Standard libraries: `string`, `table`, `math`
- `os.clock()` (wall-clock timing only)
- `os.remove()` (deletes a file inside the I/O directory)
- `print()` (captured to logs, not written to stdout)
- `json.encode()` / `json.decode()`
- `sdk.*` functions (from OpenAPI specs and upstream MCP servers)
- `io.open()`, `io.lines()`, `io.list()`, `io.type()` (sandboxed file I/O, see below)

**Conditionally available — sandboxed `io`:**

The `io` library is a sandboxed subset of Lua's standard `io`. All paths are resolved relative to a single I/O directory (default `./toolscript-files`, override with `--io-dir`). Path traversal outside this directory is rejected. In stdio mode, `io` is enabled by default. In hosted (HTTP/SSE) mode, it is disabled unless explicitly enabled via `--io-dir` or the `[io]` config section.

**Blocked:**

- `os.execute` (shell access)
- `loadfile`, `dofile`, `require` (module loading)
- `debug` library
- `string.dump` (bytecode access)
- `load` (dynamic code loading)
- Raw network access
- Unsandboxed filesystem access

**Enforcement mechanisms:**

- Luau native sandbox mode (read-only globals, isolated per-script environments)
- Configurable memory limit
- Wall-clock timeout via Luau interrupt callbacks
- API call counter per execution
- Fresh VM per execution (no state leaks between scripts)
- Credentials never exposed to Luau -- injected server-side

**A note on hosting.** If you deploy toolscript over HTTP for multiple users, you are offering your compute as a code sandbox. The sandboxing limits the abuse surface, but you should deploy behind appropriate resource constraints and network policies. For most use cases, running locally over stdio with your own credentials is the simplest and most secure option.

## Docker

Build and run:

```bash
docker build -t toolscript .
docker run toolscript https://api.example.com/openapi.json
```

For HTTP transport:

```bash
docker run -p 8080:8080 toolscript \
  https://api.example.com/openapi.json \
  --transport sse --port 8080
```

### Docker and MCP servers

The Docker image is built from `scratch` — it contains only the statically linked binary and CA certificates. This means it is designed for **hosted, HTTP-based deployments** where both the downstream transport (how clients connect to toolscript) and any upstream MCP servers use HTTP.

**HTTP upstream MCP servers work out of the box:**

```bash
docker run -p 8080:8080 toolscript \
  --mcp remote=https://mcp.example.com/mcp \
  --transport sse --port 8080
```

**Stdio upstream MCP servers will not work** in the default image. Stdio mode spawns a child process (e.g. `npx`, `python`), and those runtimes are not present in the scratch image. This is by design — stdio MCP servers are intended for single-user local scenarios where toolscript runs directly on the host, not for hosted multi-user deployments.

If you need stdio MCP servers in a container, extend the image with the required runtime:

```dockerfile
FROM node:20-slim
COPY --from=toolscript:latest /toolscript /toolscript
RUN npm install -g @modelcontextprotocol/server-filesystem
ENTRYPOINT ["/toolscript", "run"]
```

In practice, most containerized deployments should use HTTP-based upstream MCP servers. If an upstream server only supports stdio, consider running it behind an HTTP adapter or as a sidecar exposing an HTTP endpoint.

## Building from Source

```bash
git clone https://github.com/alenna/toolscript.git
cd toolscript
cargo build --release
cargo test
```

Requires Rust 1.85+ (uses edition 2024).

## License

MIT
