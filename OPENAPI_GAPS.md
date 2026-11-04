# OpenAPI V3 Feature Gap Analysis

## What's Supported Today

The project handles the core happy path for OpenAPI 3.0.x, 3.1.x, and 3.2.x:

- 5 HTTP methods (GET/POST/PUT/PATCH/DELETE)
- Path, query, and header parameters (all wired to HTTP requests)
- Local `$ref` resolution (recursive, cycle-safe)
- Basic schema types (string/number/integer/boolean/array/object)
- `allOf` composition (merges properties from all sub-schemas, nested and cycle-safe)
- `additionalProperties` as map types (`{[string]: T}` in Luau)
- `nullable` fields (rendered with `?` suffix in Luau) — both 3.0 `nullable: true` and 3.1 `type: [T, "null"]`
- `format` hints surfaced in annotation comments (uuid, date-time, int32, etc.)
- String enums with Luau union types; 3.1 `const` as a single-value enum
- Default values on parameters
- JSON request/response bodies (`application/json`)
- Response schema extraction from all 2xx status codes
- Bearer/Basic/API-Key authentication
- operationId, tags, deprecated markers, summary/description
- JSON Schema 2020-12 tolerance: boolean schemas (`true`/`false`) become permissive unknown, `prefixItems` and unknown keywords ignored
- Friendly rejection of Swagger 2.0 documents with a migration hint

## Version Handling

Each spec validates through the typed crate for its dialect, then flows through one shared pipeline:

- `openapi: "3.0.*"` → validated by `openapiv3`
- `openapi: "3.1.*"` / `"3.2.*"` → validated by `oas3` (its `~3.1` version pin is bypassed for 3.2)
- Both dialects serialize to one JSON document, normalize (`src/codegen/normalize.rs`), and extract through one version-agnostic walker (`src/codegen/walker.rs`)
- Schema interpretation (`src/codegen/luau_types.rs`) serves both OpenAPI specs and upstream MCP tool schemas

3.2 documents validate like their 3.1 core: 3.2-only constructs either fail validation (new parameter locations/styles) or drop silently (see Tier 5).

## Unsupported Features, Prioritized by Real-World Frequency

### Tier 1 — Critical (used by the vast majority of non-trivial APIs)

| # | Feature | Why it matters | Real-world prevalence |
|---|---------|---------------|----------------------|
| 1 | ~~**`allOf` composition**~~ | ✅ Done — merges properties from all sub-schemas recursively, cycle-safe. | ~80-90% of production APIs |
| 2 | **`oneOf` / `anyOf`** | Polymorphic responses, union types, flexible inputs (e.g., payment method is oneOf CreditCard, BankTransfer). Currently falls back to `String`. `FieldType::OneOf`/`AnyOf` variants are reserved (DR-005) and never produced; the walker seam makes this a small delta. | ~50-60% of APIs |
| 3 | ~~**`nullable` types**~~ | ✅ Done — 3.0 `nullable: true` and 3.1 `type: [T, "null"]` both render `T?` in Luau. | ~70%+ of APIs |
| 4 | ~~**`additionalProperties`**~~ | ✅ Done — maps render as `{[string]: T}` in Luau. | ~50-60% of APIs |
| 5 | ~~**Error response schemas (2xx range)**~~ | ✅ Done — now extracts from all 2xx status codes, not just 200/201. | ~90%+ of APIs define error responses |
| 6 | **`application/x-www-form-urlencoded` bodies** | OAuth2 token endpoints, legacy forms, payment APIs. Currently only JSON bodies are handled. | ~30-40% of APIs (nearly 100% of OAuth APIs) |

### Tier 2 — High (used by many production APIs)

| # | Feature | Why it matters | Real-world prevalence |
|---|---------|---------------|----------------------|
| 7 | **OAuth2 security schemes** | Most SaaS APIs use OAuth2 (Stripe, GitHub, Google, etc.). Currently only static token/key auth is supported. | ~40-50% of public APIs |
| 8 | ~~**Header parameters (runtime)**~~ | ✅ Done — header params now wired through to HTTP requests. | ~30-40% of APIs |
| 9 | ~~**`format` hints + runtime validation**~~ | ✅ Done — format strings surfaced in Luau annotations AND validated at runtime (uuid, date-time, date, email, uri, ipv4, ipv6, hostname, int32, int64). Enum values also enforced. | ~60-70% of schemas use format |
| 10 | **Multipart/form-data & file uploads** | File uploads, image processing APIs, document APIs. `string` + `format: binary`. | ~20-30% of APIs |
| 11 | **`readOnly` / `writeOnly`** | `id` is readOnly (don't send on create), `password` is writeOnly (never returned). Important for accurate request vs. response types. | ~30-40% of CRUD APIs |
| 12 | **Multiple servers** | Only `servers[0]` is used. APIs commonly list staging, production, and sandbox URLs. | ~30% of specs list >1 server |
| 13 | **Server variables / URL templating** | `https://{region}.api.example.com/v{version}`. Used for multi-region/versioned APIs. | ~15-20% of specs |
| 14 | **Discriminator** | Works with oneOf to route polymorphic payloads. Used by Stripe, Twilio, and others. | ~15-20% of APIs with polymorphism |
| 15 | **Per-operation security overrides** | Public endpoints (health check, docs) alongside authenticated ones. `security: []` on an operation means "no auth required." | ~25-30% of APIs |

### Tier 3 — Medium (common enough to matter)

| # | Feature | Why it matters | Real-world prevalence |
|---|---------|---------------|----------------------|
| 16 | **Array query param serialization (`explode`)** | `?id=1&id=2&id=3` vs `?id=1,2,3`. Default `form`+`explode: true` differs from what reqwest does by default. | ~20-25% of APIs with array params |
| 17 | **`deepObject` style params** | `?filter[status]=active&filter[type]=user`. Common in filtering/faceted search APIs. | ~10-15% of APIs |
| 18 | **Response headers** | Rate limit headers (`X-RateLimit-Remaining`), pagination (`Link`), ETag, Location. | ~30%+ of APIs return important headers |
| 19 | ~~**Nested/recursive `$ref` in compositions**~~ | ✅ Done for `allOf` — nested allOf resolves recursively and cyclic refs terminate with a visited set. `oneOf`/`anyOf` nesting follows #2. | Follows from #1 and #2 |
| 20 | **`default` values on schema fields** | Beyond parameters — schema property defaults affect object construction. | ~20-30% of schemas |
| 21 | **Cookie parameters** | Session-based APIs, some legacy APIs. Currently explicitly skipped. | ~5-10% of APIs |
| 22 | **`example` / `examples` objects** | Useful for generating test data, documentation, and LLM context. Parsed but not surfaced. | ~40% of well-documented APIs |
| 23 | **HEAD / OPTIONS methods** | HEAD for existence checks, OPTIONS for CORS preflight. Explicitly skipped. | ~10% of APIs define these |

### Tier 4 — Lower Priority (niche or growing)

| # | Feature | Why it matters | Real-world prevalence |
|---|---------|---------------|----------------------|
| 24 | ~~**Callbacks**~~ | 🚫 Deliberately excluded (DR-003) — callbacks are requests the API sends *to* the consumer, not callable upstream endpoints; generating `sdk.*` functions for them would mislead the LLM. | ~5-10% of APIs |
| 25 | ~~**Webhooks (3.1/3.2)**~~ | 🚫 Deliberately excluded (DR-003) — same reasoning as callbacks. 3.1/3.2 specs with top-level `webhooks` parse fine; webhooks produce no SDK functions. | ~5% of 3.1 specs |
| 26 | **Links** | Hypermedia-style API navigation (response from create -> link to get). Low adoption. | ~3-5% of APIs |
| 27 | **Remote `$ref`** | References to external files/URLs. Used in large API ecosystems with shared schemas. | ~10% of enterprise specs |
| 28 | **OpenID Connect** | `openIdConnect` security scheme with discovery URL. | ~5% of APIs |
| 29 | **`not` keyword** | Schema exclusion/negation. Rarely used in practice. | ~2-3% of APIs |
| 30 | **`patternProperties`** | Regex-keyed object properties. Very rare. | ~1-2% |
| 31 | **XML serialization hints** | `xml` object for SOAP-style APIs. Legacy. | ~5% (declining) |
| 32 | **Min/max/pattern constraints** | `minimum`, `maximum`, `minLength`, `maxLength`, `pattern`, `minItems`, `maxItems`, `uniqueItems`. Validation-oriented, less critical for SDK generation. 3.1's numeric `exclusiveMinimum`/`exclusiveMaximum` parse but are ignored. | ~30% of schemas (but low impact for codegen) |
| 33 | **`allowReserved`** | Allow reserved characters in query params without encoding. | ~1-2% |
| 34 | **Parameter `content` (vs `schema`)** | Complex serialization of parameters via media type instead of schema. | ~2-3% |

### Tier 5 — OpenAPI 3.2-Specific (dropped or rejected during validation)

oas3 validates 3.2 documents against the 3.1 model. 3.2-only constructs either fail validation or drop silently, so a 3.2 spec generates like its 3.1 core. See DR-002.

| # | Feature | What happens today | Real-world prevalence |
|---|---------|---------------|----------------------|
| 35 | **`query` HTTP method** | Operations defined under `query` drop silently. The method set (get/post/put/patch/delete) excludes them. | New in 3.2; rare in the wild |
| 36 | **`additionalOperations`** | The whole map drops silently — operations defined there never generate. | New in 3.2; rare in the wild |
| 37 | **`querystring` parameter definition** | Documents using `in: querystring` **fail validation** (hard error), because oas3's `ParameterIn` enum has no such value. | New in 3.2; rare in the wild |
| 38 | **Streaming media types + `itemSchema`** | `text/event-stream`, `application/jsonl`, `application/json-seq`, `multipart/mixed` fall through content-type selection; `itemSchema` drops silently. Streaming responses are a runtime feature, not just parsing. | New in 3.2; growing (AI/chat APIs) |
| 39 | **Tag `kind` / `parent`** | Dropped silently. Tags still work by name for grouping. | New in 3.2; rare in the wild |
| 40 | **3.2 parameter `style` values** | New style values (e.g. 3.2-only cookie styles) **fail validation** — oas3's `ParameterStyle` enum rejects them. | New in 3.2; rare in the wild |

## Recommended Implementation Order

Sequenced by impact (next items to tackle):

1. ~~**`allOf` composition**~~ ✅
2. **`oneOf` / `anyOf`** — completes polymorphism support. Even a "first match" heuristic is better than `String`. The reserved `FieldType` variants (DR-005) make this a small delta at the walker seam.
3. ~~**`nullable`**~~ ✅
4. ~~**`additionalProperties`**~~ ✅
5. ~~**Error response schemas**~~ ✅
6. **Form-encoded request bodies** — unblocks OAuth token endpoints and payment APIs.
7. ~~**Header parameters at runtime**~~ ✅
8. ~~**`format` validation (annotations + runtime)**~~ ✅
9. **OAuth2** — large class of APIs become usable.
10. **`readOnly`/`writeOnly`** — generate separate request/response types.

6 of 10 items completed. Remaining priorities: `oneOf`/`anyOf` (#2), form-encoded bodies (#6), OAuth2 (#9), `readOnly`/`writeOnly` (#10).

Tier 5 items are separate from this list: each needs either an oas3 upgrade (when upstream models the construct) or a documented pre-validation pass.

## Code References

| Area | File | Notes |
|------|------|-------|
| Spec loading + version routing | `src/codegen/parser.rs` | `sniff_version`/`validate`: 3.0→`openapiv3`, 3.1/3.2→`oas3` (pin bypassed), Swagger 2.0 rejected with migration hint. YAML/JSON loaders shared |
| Dialect normalization | `src/codegen/normalize.rs` | 3.1 type arrays → `type` + `nullable`, `const` → enum, boolean schemas → permissive. Walks schemas under paths, components, parameters, request bodies, responses |
| Schema parsing | `src/codegen/walker.rs` | `extract_schemas`: object + allOf (nested, cycle-safe) merged. oneOf/anyOf still fall back to `String` (reserved variants, DR-005) |
| Schema interpretation | `src/codegen/luau_types.rs` | Single JSON Schema interpreter for both OpenAPI specs and upstream MCP tools. Handles `const`, boolean schemas, type arrays, map types |
| Parameter extraction | `src/codegen/walker.rs` | Path, query, header all supported. Cookie params still skipped. `#/components/parameters/{name}` refs resolved |
| Request body handling | `src/codegen/walker.rs` | JSON-only, no form-encoded/multipart. `#/components/requestBodies/{name}` refs resolved |
| Response extraction | `src/codegen/walker.rs` | All 2xx status codes + default checked; `2XX` wildcard ranges ignored |
| Auth detection | `src/codegen/walker.rs` | Bearer/Basic/API-Key only. First supported inline scheme wins; oauth2/openIdConnect/mutualTLS skipped |
| Server URL | `src/codegen/walker.rs` | First server only, resolved against spec source; no variables |
| Runtime param handling | `src/runtime/registry.rs` | Path, query, and header params all wired to HTTP. Enum + format validated before request |
| Param validation | `src/runtime/validate.rs` | Enum values enforced, 10 format validators (uuid, date-time, date, email, uri, ipv4, ipv6, hostname, int32, int64) |
| HTTP layer | `src/runtime/http.rs` | Custom headers supported. All bodies still sent as application/json |
| Manifest types | `src/codegen/manifest.rs` | Map type, nullable, format fields on FieldDef and ParamDef. `OneOf`/`AnyOf` variants reserved (DR-005), lower to `string`. No readOnly/writeOnly yet |
| Luau annotations | `src/codegen/annotations.rs` | Map types rendered. No union types for oneOf yet — reserved variants lower to `string` |
