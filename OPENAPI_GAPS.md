# OpenAPI V3 Feature Gap Analysis

## What's Supported Today

The project handles the core happy path well:

- 5 HTTP methods (GET/POST/PUT/PATCH/DELETE)
- Path, query, and header parameters (all wired to HTTP requests)
- Local `$ref` resolution (recursive)
- Basic schema types (string/number/integer/boolean/array/object)
- `allOf` composition (merges properties from all sub-schemas)
- `additionalProperties` as map types (`{[string]: T}` in Luau)
- `nullable` fields (rendered with `?` suffix in Luau)
- `format` hints surfaced in annotation comments (uuid, date-time, int32, etc.)
- String enums with Luau union types
- Default values on parameters
- JSON request/response bodies (`application/json`)
- Response schema extraction from all 2xx status codes
- Bearer/Basic/API-Key authentication
- operationId, tags, deprecated markers, summary/description

## Unsupported Features, Prioritized by Real-World Frequency

### Tier 1 — Critical (used by the vast majority of non-trivial APIs)

| # | Feature | Why it matters | Real-world prevalence |
|---|---------|---------------|----------------------|
| 1 | ~~**`allOf` composition**~~ | ✅ Done — merges properties from all sub-schemas recursively. | ~80-90% of production APIs |
| 2 | **`oneOf` / `anyOf`** | Polymorphic responses, union types, flexible inputs (e.g., payment method is oneOf CreditCard, BankTransfer). Currently falls back to `String`. | ~50-60% of APIs |
| 3 | ~~**`nullable` types**~~ | ✅ Done — extracts `nullable: true` and renders as `T?` in Luau. | ~70%+ of APIs |
| 4 | ~~**`additionalProperties`**~~ | ✅ Done — maps render as `{[string]: T}` in Luau. | ~50-60% of APIs |
| 5 | ~~**Error response schemas (2xx range)**~~ | ✅ Done — now extracts from all 2xx status codes, not just 200/201. | ~90%+ of APIs define error responses |
| 6 | **`application/x-www-form-urlencoded` bodies** | OAuth2 token endpoints, legacy forms, payment APIs. Currently only JSON bodies are handled. | ~30-40% of APIs (nearly 100% of OAuth APIs) |

### Tier 2 — High (used by many production APIs)

| # | Feature | Why it matters | Real-world prevalence |
|---|---------|---------------|----------------------|
| 7 | **OAuth2 security schemes** | Most SaaS APIs use OAuth2 (Stripe, GitHub, Google, etc.). Currently only static token/key auth is supported. | ~40-50% of public APIs |
| 8 | ~~**Header parameters (runtime)**~~ | ✅ Done — header params now wired through to HTTP requests. | ~30-40% of APIs |
| 9 | ~~**`format` hints**~~ | ✅ Done — format strings surfaced in Luau annotation comments. | ~60-70% of schemas use format |
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
| 19 | **Nested/recursive `$ref` in compositions** | When allOf/oneOf reference schemas that themselves use allOf/oneOf. Common in large specs. | Follows from #1 and #2 |
| 20 | **`default` values on schema fields** | Beyond parameters — schema property defaults affect object construction. | ~20-30% of schemas |
| 21 | **Cookie parameters** | Session-based APIs, some legacy APIs. Currently explicitly skipped. | ~5-10% of APIs |
| 22 | **`example` / `examples` objects** | Useful for generating test data, documentation, and LLM context. Parsed but not surfaced. | ~40% of well-documented APIs |
| 23 | **HEAD / OPTIONS methods** | HEAD for existence checks, OPTIONS for CORS preflight. Explicitly skipped. | ~10% of APIs define these |

### Tier 4 — Lower Priority (niche or growing)

| # | Feature | Why it matters | Real-world prevalence |
|---|---------|---------------|----------------------|
| 24 | **Callbacks** | Webhook registration flows where the API calls back to the client. | ~5-10% of APIs |
| 25 | **Webhooks (3.1)** | Top-level `webhooks` field, separate from callbacks. | ~5% of 3.1 specs |
| 26 | **Links** | Hypermedia-style API navigation (response from create -> link to get). Low adoption. | ~3-5% of APIs |
| 27 | **Remote `$ref`** | References to external files/URLs. Used in large API ecosystems with shared schemas. | ~10% of enterprise specs |
| 28 | **OpenID Connect** | `openIdConnect` security scheme with discovery URL. | ~5% of APIs |
| 29 | **`not` keyword** | Schema exclusion/negation. Rarely used in practice. | ~2-3% of APIs |
| 30 | **`patternProperties`** | Regex-keyed object properties. Very rare. | ~1-2% |
| 31 | **XML serialization hints** | `xml` object for SOAP-style APIs. Legacy. | ~5% (declining) |
| 32 | **Min/max/pattern constraints** | `minimum`, `maximum`, `minLength`, `maxLength`, `pattern`, `minItems`, `maxItems`, `uniqueItems`. Validation-oriented, less critical for SDK generation. | ~30% of schemas (but low impact for codegen) |
| 33 | **`allowReserved`** | Allow reserved characters in query params without encoding. | ~1-2% |
| 34 | **Parameter `content` (vs `schema`)** | Complex serialization of parameters via media type instead of schema. | ~2-3% |

## Recommended Implementation Order

Sequenced by impact (next items to tackle):

1. ~~**`allOf` composition**~~ ✅
2. **`oneOf` / `anyOf`** — completes polymorphism support. Even a "first match" heuristic is better than `String`.
3. ~~**`nullable`**~~ ✅
4. ~~**`additionalProperties`**~~ ✅
5. ~~**Error response schemas**~~ ✅
6. **Form-encoded request bodies** — unblocks OAuth token endpoints and payment APIs.
7. ~~**Header parameters at runtime**~~ ✅
8. ~~**`format` pass-through**~~ ✅
9. **OAuth2** — large class of APIs become usable.
10. **`readOnly`/`writeOnly`** — generate separate request/response types.

6 of 10 items completed. Remaining priorities: `oneOf`/`anyOf` (#2), form-encoded bodies (#6), OAuth2 (#9), `readOnly`/`writeOnly` (#10).

## Code References

| Area | File | Notes |
|------|------|-------|
| Schema parsing | `src/codegen/parser.rs` | allOf merged, additionalProperties→Map, nullable/format extracted. oneOf/anyOf still fall back to String |
| Parameter extraction | `src/codegen/parser.rs` | Path, query, header all supported. Cookie params still skipped |
| Request body handling | `src/codegen/parser.rs` | JSON-only, no form-encoded/multipart |
| Response extraction | `src/codegen/parser.rs` | All 2xx status codes + default checked |
| Auth detection | `src/codegen/parser.rs` | Bearer/Basic/API-Key only |
| Server URL | `src/codegen/parser.rs` | First server only, no variables |
| Runtime param handling | `src/runtime/registry.rs` | Path, query, and header params all wired to HTTP |
| HTTP layer | `src/runtime/http.rs` | Custom headers supported. All bodies still sent as application/json |
| Manifest types | `src/codegen/manifest.rs` | Map type, nullable, format fields added. No readOnly/writeOnly yet |
| Luau annotations | `src/codegen/annotations.rs` | Map types rendered. No union types for oneOf yet |
