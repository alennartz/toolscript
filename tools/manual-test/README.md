# Manual Test Tools

Reusable drivers for manual testing of toolscript. Journeys live in `PLAN.md`.

## mcp_stdio.py

A minimal MCP stdio client. Spawns a toolscript process, speaks the stdio MCP
protocol (initialize → notifications/initialized), and runs a scenario of tool
calls and resource reads.

```
e2e/.venv/bin/python tools/manual-test/mcp_stdio.py \
    <toolscript-cmd-and-args> [--timeout <sec>] < scenario.jsonl
```

- **Input:** scenario on stdin, one JSON object per line.
- **Output:** one JSON result object per line on stdout.
- **Exit code:** 1 if any op errored, else 0.
- **Requires:** Python 3 with the `json`/`subprocess` stdlib only.

Ops:

| op | arguments | notes |
|---|---|---|
| `list_apis` | — | — |
| `list_functions` | `api` (optional) | passes the filter through |
| `get_function_docs` | `name` | bare function name, e.g. `list_pets` |
| `search_docs` | `query` | — |
| `execute_script` | `script` | Luau source |
| `read_resource` | `uri` | e.g. `sdk://petstore/overview` |

Toolscript errors (transport-level) surface as `{"ERROR": "..."}` in the result
object and fail the run. Script-level errors come back inside the
`execute_script` result text.

## run_local_upstream.py

Starts the FastAPI test API from `e2e/test_api/` on a free local port as a live
upstream for `execute_script` journeys.

```
e2e/.venv/bin/python tools/manual-test/run_local_upstream.py
```

- **Output:** base URL on stdout, then blocks until stdin closes.
- **Use:** run it in the background reading from `sleep infinity |` (or a FIFO),
  capture the URL, pass it to toolscript as the spec source. The seeded bearer
  token is `test-secret-123` (`--auth name:TEST_API_BEARER_TOKEN` with
  `TEST_API_BEARER_TOKEN=test-secret-123` in the server's environment).
- **Reset:** POST `/reset` clears CRUD state between runs.
