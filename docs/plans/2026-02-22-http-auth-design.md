# HTTP Transport Authentication Design

## Overview

Add authentication and upstream credential handling to code-mcp's HTTP/SSE transport. Two independent layers:

1. **MCP layer** — OAuth 2.1 resource server. Validates client identity via JWT (audience + issuer). No scopes or RBAC.
2. **Upstream layer** — Client-provided credentials via `_meta.auth` on tool calls. Falls back to server-side env vars.

The stdio transport is unaffected.

## Problem

The HTTP transport currently accepts all connections without authentication. When the server is hosted remotely, anyone who can reach the port can invoke all tools — including `execute_script`, which makes authenticated API calls using the server's static credentials. All users share the same upstream credentials loaded from environment variables at startup.

## Architecture

```
MCP Client                    code-mcp (HTTP)                 Upstream API
    │                              │                              │
    │── Authorization: Bearer JWT ─→│                              │
    │   _meta.auth.petstore: sk-.. │                              │
    │                              │── validate JWT ──→ JWKS      │
    │                              │── check aud + iss ──→ ok?    │
    │                              │                              │
    │                              │── inject sk-.. as Bearer ───→│
    │                              │←── API response ─────────────│
    │←── MCP tool result ──────────│                              │
```

## Layer 1: MCP Client Authentication

### Middleware

A tower middleware layer sits in front of `StreamableHttpService` in the axum router. On every request to `/mcp`:

1. Extract `Authorization: Bearer <token>` from request headers
2. Validate the JWT:
   - Signature verification against the authorization server's JWKS (fetched via OIDC discovery, cached)
   - Expiry check (`exp` claim)
   - Audience validation (`aud` must contain the configured audience string)
   - Issuer validation (`iss` must match the configured authority)
3. Extract `sub` claim into `AuthContext`
4. Insert `AuthContext` into `http::Request::extensions_mut()`
5. If validation fails → HTTP 401 with `WWW-Authenticate` header

### AuthContext

```rust
#[derive(Clone, Debug)]
pub struct AuthContext {
    pub subject: String,  // user ID from JWT sub claim
}
```

Stored in http request extensions. Flows through rmcp's `StreamableHttpService` into tool handlers via `Extension<Parts>` → `parts.extensions.get::<AuthContext>()`.

### JWKS Handling

- On first request (or startup), fetch OIDC discovery document from `{authority}/.well-known/openid-configuration`
- Extract `jwks_uri` from the discovery response
- Fetch and cache the JWKS
- Refresh on cache miss (unknown key ID) or periodically

### Well-Known Endpoint

`GET /.well-known/oauth-protected-resource` served by axum outside the `/mcp` nest:

```json
{
  "resource": "https://mcp.example.com",
  "authorization_servers": ["https://auth.example.com"]
}
```

### Configuration

CLI flags:

```bash
code-mcp serve ./output/ --transport sse --port 8080 \
  --auth-authority https://auth.example.com \
  --auth-audience https://mcp.example.com
```

Optional explicit JWKS URI override (skips OIDC discovery):

```bash
  --auth-jwks-uri https://auth.example.com/.well-known/jwks.json
```

Environment variable equivalents:

```
MCP_AUTH_AUTHORITY=https://auth.example.com
MCP_AUTH_AUDIENCE=https://mcp.example.com
MCP_AUTH_JWKS_URI=https://auth.example.com/.well-known/jwks.json
```

When no auth config is provided, the middleware is not added and the server behaves as it does today (open access). This preserves backward compatibility.

## Layer 2: Upstream Credential Injection

### _meta.auth

The `execute_script` tool handler reads upstream API credentials from the MCP protocol's `_meta` field on the tool call request. `_meta` is injected by the MCP client at the transport layer and is invisible to the LLM.

Format:

```json
{
  "method": "tools/call",
  "params": {
    "name": "execute_script",
    "arguments": { "script": "local pets = sdk.list_pets()\nreturn pets" },
    "_meta": {
      "auth": {
        "petstore": { "type": "bearer", "token": "sk-user-secret" },
        "billing": { "type": "api_key", "key": "billing-key-123" },
        "legacy": { "type": "basic", "username": "user", "password": "pass" }
      }
    }
  }
}
```

The `type` field matches the manifest's `AuthConfig` enum variants: `bearer`, `api_key`, `basic`.

### Credential Resolution Order

For each API referenced during script execution:

1. `_meta.auth.<api_name>` — per-request, from client (takes precedence)
2. Server-side env vars (`{API_NAME}_BEARER_TOKEN`, etc.) — fallback

### Implementation

- The `execute_script` tool handler extracts `Meta` from the request context (rmcp provides this via the `FromContextPart` trait)
- Parses `meta.get("auth")` into an `AuthCredentialsMap`
- Merges with server-side env var credentials (client overrides server)
- Passes the merged map to `ScriptExecutor::execute()`

The `ScriptExecutor` already accepts `AuthCredentialsMap`. The change is that it receives a per-execution merged map instead of the server-wide static map.

### rmcp Support

rmcp 0.16 has full `_meta` support:

- `Meta` is a transparent wrapper around `serde_json::Map<String, Value>`
- Tool handlers can extract `Meta` directly as a function parameter via `FromContextPart`
- `_meta` is deserialized from `tools/call` params automatically

## What Doesn't Change

- **stdio transport** — completely unaffected, continues using env var credentials
- **`load_auth_from_env()`** — still works as server-side default credentials
- **Lua sandbox** — still never sees credentials (injected by Rust at HTTP call time)
- **Manifest format** — unchanged
- **Read-only tools** (`list_apis`, `list_functions`, `get_function_docs`, `search_docs`, `get_schema`) — no upstream calls, only gated by MCP-level auth if configured

## New Dependencies

- `jsonwebtoken` — JWT decoding and validation
- `tower` — middleware layer (already implicit via axum, may need explicit dep for `ServiceBuilder`)

## Security Properties

- **MCP client authentication**: JWT audience + issuer validation prevents unauthorized access to the HTTP transport
- **Credential isolation**: Upstream credentials in `_meta` are per-request and per-user. Different users provide different credentials.
- **Lua sandbox unchanged**: Credentials never enter the Lua environment. Rust injects them at the HTTP request boundary.
- **Fallback path**: Env var credentials still work for stdio and for HTTP deployments where the operator provides server-side credentials (e.g., internal tools with a shared service account).
- **No token passthrough in the MCP spec sense**: The MCP server validates its own JWT and does not forward it upstream. Upstream credentials are a separate concern provided by the client via `_meta`.
