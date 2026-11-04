# Manual Testing — 2026-11-03-openapi-31-support

## Smoke Suite

All four journeys from `tools/manual-test/PLAN.md`, each driven against the new
3.1/3.2 pipeline and the unchanged 3.0 pipeline as the regression twin:

- **Journey 1 (generate)** — `toolscript generate` over
  `testdata/petstore_31.yaml`, `testdata/petstore_32.yaml`, and
  `testdata/petstore.yaml` (3.0 regression), plus output coherence review
  (manifest + `sdk/pets.luau`).
- **Journey 2 (serve + discover)** — stdio MCP server over the same three specs;
  `list_apis`, `list_functions`, `get_function_docs` on a function whose schema
  carries a nullable field (`get_pet_docs`, `tag` field, `type: [string, "null"]`
  in 3.1/3.2 vs `nullable: true` in 3.0).
- **Journey 3 (execute_script end-to-end)** — Luau script chaining SDK calls
  against the live FastAPI upstream from `e2e/test_api/`: list + create + get +
  filter + `print()` capture. The upstream spec is generated live from the
  FastAPI app, so this journey exercises the generation path on a spec that is
  not one of the checked-in fixtures.
- **Journey 4 (unfriendly input)** — Swagger 2.0 document through `toolscript
  generate` must produce the friendly migration error, exit code 1, and no
  output directory.

Focus-hint-derived adjacent concerns (added as topic-specific tests): URL spec
loading (loading from an http:// URL is the sibling path of file loading in
`load_spec`), the missing-file error path, and stdout hygiene of `toolscript
run` (the stdio MCP channel shares stdout with the process).

## Topic-Specific Tests

1. **Dialect twin manifests.** `generate` on petstore_31.yaml and
   petstore_32.yaml produce byte-identical manifests and byte-identical SDK
   files; petstore.yaml (3.0) produces the same functions/schemas modulo the
   spec's own description prose. Why: the whole topic promises dialect
   equivalence at the user surface. Tool: `bash` + `diff`.

2. **Nullable field coherence across dialects.** `get_function_docs` on
   `get_pet_docs` (petstore fixtures) shows `tag: string?` for 3.1, 3.2, and 3.0
   alike, with identical descriptions. Why: `type: [string, "null"]` →
   `nullable: true` normalization is the single most common 3.1 construct and
   the original parse failure. Tool: `mcp_stdio.py`.

3. **3.2 degradation.** The 3.2 fixture's `query` method and webhook produce no
   SDK functions (the spec's own surface: three paths, one query op, one
   webhook). Why: deliberate-exclusion behavior must degrade silently, not
   error. Tool: `mcp_stdio.py list_functions` + manifest inspection.

4. **URL spec loading (3.1 over http).** `toolscript generate
   http://127.0.0.1:PORT/openapi_31.yaml` against a local static file server
   produces the same manifest as the file-based run. Why: URL loading is the
   sibling path of file loading in the new `load_spec`; the fixtures carry no
   `servers` URL so the relative-URL resolution rule is exercised only via a
   remote source. Tool: `python3 -m http.server` + `bash`.

5. **Missing file error.** `toolscript generate /nonexistent.yaml` errors
   clearly. Why: adjacent error path of the loader. Tool: `bash`.

6. **stdout hygiene of `run`.** `toolscript run` prints nothing but the MCP
   protocol to stdout (stderr carries the startup message). Why: a stray print
   on stdout corrupts the stdio transport; the serve pipeline was rewired
   around the new parser entry points. Tool: `bash` + grep.

7. **Real-world 3.1 doc through the full pipeline.** Generate from a genuine
   3.1 fixture not authored for this repo. Why: the plan's fog note — real-world
   3.1 specs may surface constructs the repo fixtures do not. Tool: `bash`.

## Tools

- Reused: `e2e/test_api/` FastAPI app (live upstream), `e2e/.venv` (provides
  the `mcp` package dependency check for the venv; the driver itself needs only
  the stdlib).
- New: `tools/manual-test/mcp_stdio.py` (generic stdio MCP client/scenario
  runner), `tools/manual-test/run_local_upstream.py` (live upstream launcher).
- Improved: none (first run).

## Harness Limitations

- The `execute_script` journey runs against the FastAPI test API, a real HTTP
  server on localhost — not a mock: real TCP, real auth header injection, real
  JSON serialization. Limitations: it is single-user, seeded, and its OpenAPI
  document is FastAPI-generated 3.1 (downgraded to 3.0 by the app), so it
  cannot surface concurrency or real-internet-TLS issues.
- The URL-loading test uses a local static file server, not HTTPS; no TLS
  behavior is exercised. (Pre-existing loader code, out of this topic's diff.)
- The real-world 3.1 test depends on an internet fetch; if the network is
  unavailable it degrades to the in-repo fixtures only (recorded in Results).
- stdio framing is exercised by a hand-rolled client, not the official MCP SDK
  client. The e2e suite already covers the SDK client path; the hand-rolled
  client covers raw-protocol hygiene (newline-delimited framing) which the SDK
  client would mask.
- None of these gaps weaken the topic's primary behavior: dialect routing,
  normalization, and extraction all sit behind `generate`/`run` and are fully
  observable through the real CLI + real server + real HTTP upstream.

## Results

(filled after execution)

## Plan Updates

- None this run. All four journeys were already promoted at bootstrap; the
  topic adds no new primary journey (3.1 support widens an input format, it
  does not create a new user path).

## Open Issues

(filled after execution)
