# Manual Test Plan

Persistent manual test plan for toolscript. Primary user journeys — the high-value
happy paths a human would exercise. Topic-specific edge cases live in
`docs/manual-tests/<topic>.md`, not here. Tools are indexed in `README.md`.

## Journey 1: Generate an SDK from an OpenAPI spec

**What:** `toolscript generate <spec>` produces `manifest.json` and `sdk/*.luau`
from a local OpenAPI document.

**Why it matters:** the entry point for every other journey. If generation from a
file fails or produces a broken SDK, nothing downstream can work.

**Tool:** direct shell invocation via `bash` (`target/release/toolscript generate`).

## Journey 2: Serve the MCP server and discover the SDK

**What:** `toolscript run <spec> --transport stdio`, then an MCP client connects
and calls `list_apis`, `list_functions`, and `get_function_docs` to explore what
the SDK offers.

**Why it matters:** discovery is the first thing an LLM consumer does. Wrong
names, missing functions, or unreadable docs break every downstream script.

**Tool:** `tools/manual-test/mcp_stdio.py` — a generic JSON-RPC MCP client
speaking the stdio MCP protocol against a spawned toolscript process.

## Journey 3: Execute a script end-to-end against a live API

**What:** over stdio, `execute_script` runs Luau that chains SDK calls —
including a call that hits a real upstream HTTP endpoint — and returns the
result plus captured `print()` logs.

**Why it matters:** this is the product's reason to exist: one round-trip that
runs several authenticated upstream calls. Sandbox, registry, HTTP client, and
auth injection all participate; a failure here is a product-level failure.

**Tool:** `tools/manual-test/mcp_stdio.py` + a local HTTP API (the FastAPI app
in `e2e/test_api/`, driven by `tools/manual-test/run_local_upstream.py`).

## Journey 4: Unfriendly-input guidance

**What:** the CLI rejects a spec it cannot use with a clear, actionable error —
Swagger 2.0, malformed documents, missing files.

**Why it matters:** the error is the user interface for the failure mode. Cryptic
serde output was the original complaint that motivated version routing.

**Tool:** direct shell invocation via `bash`; assert on stderr and exit code.
