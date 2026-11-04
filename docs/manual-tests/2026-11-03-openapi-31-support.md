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
  carries a nullable field (`get_pet_by_id`, `tag` field, `type: [string, "null"]`
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
   `get_pet_by_id` and `create_pet` (petstore fixtures) shows `tag: string?` for
   3.1, 3.2, and 3.0 alike, with identical descriptions. Why: `type: [string,
   "null"]` → `nullable: true` normalization is the single most common 3.1
   construct and the original parse failure. Tool: `mcp_stdio.py`.

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

- Reused: `e2e/test_api/` FastAPI app (live upstream), `e2e/.venv`, `mcp` SDK
  client (against the topic's own HTTP-transport surface), direct `bash`.
- New: `tools/manual-test/mcp_stdio.py` (generic stdio MCP client/scenario
  runner), `tools/manual-test/run_local_upstream.py` (live upstream launcher).
- Improved: `tools/manual-test/mcp_stdio.py` gained a settle delay after the
  `initialized` notification (rmcp's async handshake otherwise races the first
  request) — committed as an inline fix during execution.

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

All verdicts follow the skill's pass | fixed-inline | open scale.

### Smoke Suite

| Journey | Invocation | Observed | Verdict | Coherence |
|---|---|---|---|---|
| J1 generate 3.1 | `generate testdata/petstore_31.yaml` | exit 0; `manifest.json` + `sdk/{_meta,pets}.luau`; manifest carries 3 functions (`list_pets`/`create_pet`/`get_pet_by_id`), Pet/NewPet schemas, Pet.tag nullable, bearer auth, status enum, limit default 20 | **pass** | looks coherent |
| J1 generate 3.2 | `generate testdata/petstore_32.yaml` | byte-identical manifest and SDK files to the 3.1 output; query method and webhook produce nothing | **pass** | looks coherent |
| J1 generate 3.0 | `generate testdata/petstore.yaml` | identical functions and schemas to the 3.1 output; only the description prose differs (mirrors fixture text) | **pass** | looks coherent |
| J2 discover 3.1 | `run` + mcp_stdio | list_apis shows petstore + luau builtins; list_functions exactly 3 SDK functions; get_function_docs on `get_pet_by_id` renders `tag: string?` with full schema docs | **pass** | looks coherent |
| J2 discover 3.2 | `run` + mcp_stdio | same 3 functions (not the spec's `query` method or webhook); nullable docs identical to 3.1; `sdk://petstore/overview` resource renders | **pass** | looks coherent |
| J2 discover 3.0 | `run` + mcp_stdio | identical discovery surface to 3.1/3.2; `tag: string?` in identical positions | **pass** | looks coherent |
| J3 end-to-end | `run <live-upstream-URL>` + mcp_stdio `execute_script` | one script chains `list_pets(limit=3)` -> `get_pet(id=1)` -> `create_pet(status="active")` with `print()`: seeded total 4, first pet Fido (tag "dog"), created id 5, `fetched 3 pets` in logs. All through real HTTP against the FastAPI upstream with bearer auth injected server-side | **pass** | looks coherent |
| J4 swagger 2.0 | `bash` | friendly `Error: Swagger 2.0 is not supported; convert to OpenAPI 3.x`, exit 1 | **fixed-inline** (see note) | looks coherent |

J4 note: the friendly error was correct on first run, but the failed
generation left an empty `output/sdk/` directory behind — the generator
created the output tree before parsing. Fixed inline in `generate.rs`
(commit `fix: leave no empty output directory when generation fails
(manual-testing)`): directories are now created only after every spec parses.
Re-ran J1 and J4 clean; `cargo fmt --check`, `cargo clippy -- -D warnings`,
and the full `cargo test` suite (466 tests) pass.

### Topic-Specific Tests

| Test | Invocation | Observed | Verdict |
|---|---|---|---|
| T1 dialect-twin manifests | `diff` across the three generate outputs | 3.1 and 3.2 byte-identical; 3.0 differs only in description prose (matches the fixtures' own prose difference) | **pass** |
| T2 nullable coherence | mcp_stdio `get_function_docs` on `create_pet` across dialects | `tag: string?` renders in the same two positions (`NewPet` + `Pet`) for 3.1, 3.2, and 3.0 | **pass** |
| T3 3.2 silent degradation | `generate` + mcp_stdio `list_functions` on petstore_32 | the 3.2 spec (query method + webhook) yields exactly the three core functions; zero errors | **pass** |
| T7 real-world 3.1 | `generate` on the OpenAI API spec fetched from the internet | genuine 3.1, 245 paths; exit 0 with 388 functions and 1803 schemas; nullable type-arrays render as `string?` throughout; relative-server-URL warning fires as designed for remote sources | **pass** |

Items 4, 5, and 6 from the Topic-Specific Tests list are covered by the
adjacent-checks table (they share drivers with the smoke suite). No **open**
items.

### Adjacent checks (focus-hint ring + review findings)

| Check | Observed | Verdict |
|---|---|---|
| URL spec loading (3.1 over http) | `generate http://127.0.0.1:PORT/petstore_31.yaml` byte-identical to the file-loaded manifest; `execute_script` against it reaches `list_pets` | **pass** |
| Missing file | `Error: Failed to read /nonexistent/spec.yaml` / `No such file or directory`, exit 1 | **pass** |
| Malformed YAML | clear dual YAML/JSON parse error naming both causes, exit 1 | **pass** |
| Missing `openapi` field | `Error: Missing \`openapi\` version field; not an OpenAPI 3.x document`, exit 1 | **pass** |
| stdout hygiene of `run` (5 runs + live scenario) | stdout stays pure MCP protocol; startup messages on stderr only; script `print()` captured to logs, never to process stdout | **pass** |
| search_docs over 3.1 | name/summary/description hits for "pet" with typed context lines | **pass** |
| `serve` subcommand with a 3.1 manifest | list_apis and get_function_docs work from `serve <output>`; `tag: string?` intact | **pass** |
| HTTP transport coherence | `run --transport sse` accepts a real `mcp` SDK client initialize; a browser GET of `/mcp` renders the human-readable refusal "Not Acceptable: Client must accept text/event-stream" — correct for a protocol endpoint with no web UI | **pass** |
| Review finding 4 (README claim vs behavior) | README says specs with `querystring` params or 3.2-only `style` values fail validation; the code rejects them; consistent | **pass** |

## Plan Updates

- None this run. All four journeys were already promoted at bootstrap; the
  topic adds no new primary journey (3.1 support widens an input format, it
  does not create a new user path).

## Open Issues

None. Every smoke-suite and topic-specific item passed or was fixed inline
(two inline fixes, each its own commit).
