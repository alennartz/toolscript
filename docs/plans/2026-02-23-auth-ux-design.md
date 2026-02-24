# Auth UX Redesign

## Problem

The current auth mechanism for local MCP mode requires users to:
1. Know the OpenAPI spec's `info.title`
2. Mentally apply sanitization rules (lowercase, replace non-alnum with `_`, collapse, trim)
3. Set env vars with that derived prefix (e.g. `PETSTORE_BEARER_TOKEN`)

This is not discoverable and adds unnecessary ceremony. The quickstart should be: point at a spec, provide a key, go.

## Design

### CLI Syntax

The `specs` positional argument now accepts optional `name=source` syntax:

```bash
# Auto-derive name from spec title
code-mcp run spec.yaml

# Explicit name
code-mcp run petstore=https://petstore.example.com/spec.json

# Multiple specs, each named
code-mcp run petstore=petstore.yaml billing=billing.yaml
```

New `--auth` flag (repeatable):

```bash
# name:ENV_VAR -- reads value from environment
--auth petstore:MY_PET_TOKEN

# ENV_VAR alone -- applies to the only spec (error if multiple)
--auth MY_TOKEN
```

The tool reads the value of the environment variable at startup. The secret never appears in the command itself.

New `--config` flag:

```bash
code-mcp run --config code-mcp.toml
```

When `--config` is used, positional `specs` args are disallowed.

Auto-discovery: if no `specs` and no `--config` are given, the tool looks for `code-mcp.toml` in the current directory.

### Config File Format

`code-mcp.toml`:

```toml
[apis.petstore]
spec = "https://petstore3.swagger.io/api/v3/openapi.json"
auth = "sk-my-petstore-token"

[apis.stripe]
spec = "./stripe-openapi.yaml"
auth = "sk_live_abc123"

[apis.legacy]
spec = "./legacy.yaml"
auth = { type = "basic", username = "admin", password = "secret" }
```

Rules:
- `auth = "string"` -- direct token/key value. The spec's security scheme determines injection method (bearer header vs api key header).
- `auth = { type = "basic", username = "...", password = "..." }` -- basic auth.
- `auth_env = "ENV_VAR"` -- alternative that reads from environment (for sharing config across machines).
- Section key (e.g. `petstore`) is the user-chosen API name.
- `spec` is a URL or file path.

### Auth Resolution Order

Per-API, first match wins:

1. **CLI `--auth`** -- env var reference from command line
2. **Config file `auth`** -- direct value or env reference from TOML
3. **Per-request `_meta.auth`** -- runtime override from MCP client (hosted mode only, unchanged)

The legacy `{DERIVED_NAME}_BEARER_TOKEN` convention is removed.

### API Naming

1. **Explicit name** from `name=spec` CLI syntax or config file section key.
2. **Auto-derived** from `info.title` only when a single unnamed spec is given.

For multi-spec, explicit names are required (either via CLI `name=spec` or config file).

### Error Handling

- **No auth but spec declares security scheme:** Warning at startup: `"petstore: spec declares bearer auth but no credentials configured. API calls will likely fail with 401."`
- **`--auth` name doesn't match any spec:** Error: `"--auth billing:MY_KEY but no spec named 'billing' was loaded"`
- **`--auth VALUE` (no name) with multiple specs:** Error: `"--auth without a name prefix requires exactly one spec"`
- **Config file not found:** Error with the tried path.
- **`--config` used with positional specs:** Error: `"cannot use --config with positional spec arguments"`

### What Changes

- `load_auth_from_env()` in `src/runtime/http.rs` is removed (no more auto-derived env var convention).
- `derive_api_name()` in `src/codegen/generate.rs` becomes a fallback only for single unnamed specs.
- New `src/config.rs` module for TOML config parsing.
- CLI structs in `src/cli.rs` get `--auth` and `--config` flags.
- `main.rs` auth resolution logic is rewritten.
- README auth documentation updated.

### What Stays the Same

- `_meta.auth` per-request mechanism (hosted/HTTP mode).
- MCP-layer JWT auth (`--auth-authority`, `--auth-audience`).
- Auth injection into HTTP requests (`inject_auth` in `src/runtime/http.rs`).
- The `AuthCredentials` enum and `AuthCredentialsMap` type.
- The manifest format and `AuthConfig` (bearer/apikey/basic) extracted from specs.

### Quickstart After This Change

```bash
# Simplest possible: one spec, one token
export MY_TOKEN=sk-123
code-mcp run petstore=spec.yaml --auth petstore:MY_TOKEN

# Even simpler for single-spec
export MY_TOKEN=sk-123
code-mcp run spec.yaml --auth MY_TOKEN

# Config file for persistent/multi-spec setups
code-mcp run --config code-mcp.toml

# Or just have code-mcp.toml in the current directory
code-mcp run
```
