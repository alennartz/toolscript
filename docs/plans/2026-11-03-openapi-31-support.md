# Plan: OpenAPI 3.1 / 3.2 Support

## Context

toolscript must generate SDKs from OpenAPI 3.1.x and 3.2.x specs alongside 3.0.x. Today most 3.1 specs fail at parse time because `openapiv3` rejects JSON Schema 2020-12 constructs (`type: ["string","null"]` above all). See `docs/brainstorms/openapi-31-support.md` for the exploration, crate research, and reasoning. Brainstorm decisions carried into this plan: dual parsing with a normalization seam (refined below), oneOf/anyOf reserved but follow-up, webhooks skipped, no new features in this pass.

## Architecture

### Core Insight

The dialect seam already exists in embryo. `luau_types.rs` (the MCP-tool JSON Schema converter) already interprets raw JSON schema values with 2020-12 tolerance: `type` as string or array (stripping `"null"`), 3.0 `nullable`, enums, formats, `additionalProperties`. Only the *document traversal* in `parser.rs` is typed on `openapiv3`. The design therefore:

1. Validates each spec with the typed crate for its dialect (parse errors stay precise),
2. Serializes the validated document back to one JSON `Value`,
3. Normalizes schema nodes across dialects,
4. Extracts the manifest with **one JSON document walker** shared by all versions.

Verified by probe (see Technology Choices for roundtrip evidence): both crates preserve everything the manifest needs through JSON serialization — type arrays, `const`, boolean schemas, `nullable`, param defaults, auth schemes, server variables, `2XX` response ranges, `allOf`.

### Impacted Modules

- **Codegen** — the whole change lives here.
  - `src/codegen/parser.rs`: replaced at the document level. The openapiv3-typed traversal (paths → operations → parameters/bodies/responses/auth/schemas) becomes a version-agnostic JSON walker with the same outputs. `spec_to_manifest` keeps its signature but takes the raw spec text (or pre-parsed `Value`) instead of `openapiv3::OpenAPI`. Version sniffing and dialect validation enter here. All 49 existing tests stay, re-pointed at the new API.
  - `src/codegen/luau_types.rs`: promoted from "MCP JSON converter" to the single schema interpreter used by both codegen and the walker. Gains `const` → single-value enum and boolean-schema handling. Its MCP entry points (`json_schema_to_params`, `extract_schema_defs`) keep working unchanged.
  - `src/codegen/annotations.rs`, `src/codegen/manifest.rs`: unchanged (manifest types already are the IR; `OneOf`/`AnyOf` reserved variants added to `FieldType` for the follow-up, lowering to `String`).
  - `src/codegen/generate.rs`: `derive_api_name` reads `info.title` from the JSON `Value`; `openapiv3::OpenAPI` disappears from its imports.
- **Server / Runtime** — untouched. They consume `Manifest` only.
- **testdata** — new 3.1 and 3.2 fixture specs mirroring `petstore.yaml` and `advanced.yaml` (same API, 2020-12 idioms), used for equivalence tests.

### New Modules

- `src/codegen/normalize.rs` — dialect normalization over schema `Value` nodes: 3.1 `type: [T, "null"]` → `type: T` + `nullable: true`; `const: v` → `enum: [v]`; boolean schemas (`true`/`false`) → `{}` (permissive unknown); leaves 3.0 nodes untouched. Pure functions, no I/O. Recursion via `$ref`-blind subtree walk (references resolve at extraction, as today). Normalize component parameters and request bodies as well as inline schemas.
- `src/codegen/walker.rs` — the JSON document walker: walks paths → operations → parameters/requestBody/responses/auth/components and produces `FunctionDef`/`ParamDef`/`RequestBodyDef`/`SchemaDef`/`ApiConfig` via `luau_types` + `normalize`. Owns response selection (first `2xx` in numeric order, then `default`), method mapping, name derivation, and server-URL resolution (ported unchanged from `parser.rs`).

`parser.rs` keeps: `load_spec_from_file`, `load_spec_from_url` (now returning raw text/`Value`), version sniffing/routing, and Swagger 2.0 detection with the friendly error. Its openapiv3 traversal and helpers migrate into `walker.rs` logic or die.

### Interfaces

```rust
// src/codegen/parser.rs — routing and loading (text stays raw for YAML/JSON tolerance)
pub enum SpecVersion { V30, V31Plus }   // 3.2.x routes here too

pub fn sniff_version(doc: &serde_json::Value) -> Result<SpecVersion>;
// "openapi": "3.0.*" -> V30; "3.1.*"/"3.2.*" -> V31Plus;
// top-level "swagger" key -> Err("Swagger 2.0 is not supported; convert to OpenAPI 3.x")
// missing/invalid -> Err with the observed value

pub fn validate(doc: &serde_json::Value, version: SpecVersion) -> Result<()>;
// V30: serde_json::from_value::<openapiv3::OpenAPI>, discard result (throws precise errors)
// V31Plus: serde_json::from_value::<oas3::OpenApiV3Spec>, discard result (bypasses oas3::validate_version)
// YAML input: parsed once to Value via serde_yaml, never re-serialized through oas3's yaml feature

pub async fn load_spec(path_or_url: &str) -> Result<serde_json::Value>;
// file/URL, YAML-first-then-JSON as today; returns the unnormalized document

pub fn spec_to_manifest(doc: &serde_json::Value, api_name: &str, spec_source: Option<&str>) -> Result<Manifest>;
// validate -> normalize -> walker::extract; same name, same callers (generate.rs, tests)
```

```rust
// src/codegen/normalize.rs
pub fn normalize_document(doc: &mut serde_json::Value);
// walks components/schemas, components/parameters, components/requestBodies,
// path-level and operation-level parameter schemas, requestBody/response content
// schemas; applies normalize_schema_node to each

pub fn normalize_schema_node(node: &mut serde_json::Value);
// type array with exactly one non-null member -> type: member (+ nullable: true if "null" present)
//   ["string","null"] -> {"type":"string","nullable":true}; bare ["null"] or empty -> {}
// "const": v -> {"enum": [v]}; boolean -> {}; already-3.0 nodes pass through unchanged
```

```rust
// src/codegen/walker.rs — one walker, all versions (inputs are normalized JSON)
pub fn extract_api_config(doc: &Value, api_name: &str, spec_source: Option<&str>) -> ApiConfig;
// servers[0].url resolved against spec_source (ported rule: relative URL + remote source -> join; local -> warn, keep raw)

pub fn extract_functions(doc: &Value, api_name: &str) -> Result<Vec<FunctionDef>>;
// methods: get/post/put/patch/delete; all others skipped — options/head/trace (as today),
// and 3.2 `query` (excluded on purpose: not in the shipped method set; a 3.2 spec's query
// operations degrade silently, recorded as out of scope)
// operationId -> camel_to_snake, else method+path fallback (ported)
// per-operation: parameters (item or #/components/parameters/{name}), requestBody
// (item or #/components/requestBodies/{name}; json content first, else first;
// unresolvable references error), response schema (first numeric 2xx code with
// application/json, $ref name or array items name; else default). Ignore 2XX ranges.

pub fn extract_parameters(params: &Value, doc: &Value) -> Result<Vec<ParamDef>>;
// skip cookie params; query/header/path -> ParamLocation; type from schema node via luau_types
// (integer/number/boolean/else string), default/enum/format from schema node

pub fn extract_schemas(doc: &Value) -> Vec<SchemaDef>;
// components/schemas items: object schemas -> fields; allOf -> merged properties + union required (ported rule);
// oneOf/anyOf/not top-level schemas skipped (reserved for follow-up)

pub fn extract_auth_config(doc: &Value) -> Option<AuthConfig>;
// first supported inline scheme: http bearer -> Bearer, http basic -> Basic,
// apiKey (any `in`) -> ApiKey{header: name}; skip references and unsupported schemes,
// including oauth2/openIdConnect/mutualTLS (unchanged behavior)
```

Contract for callers: `generate.rs` calls `parser::load_spec` → `parser::spec_to_manifest`; both exist for file and URL sources. Existing manifest JSON shape is unchanged (no new fields except the two reserved `FieldType` variants, which only appear for follow-up work and never for valid input today).

### Technology Choices

- **Add `oas3 = "0.22"`** (default features) for 3.1/3.2 validation. Chosen over: full JSON-Schema parsing with no typed validation (loses precise error messages), and `oas31` (immature). oas3 0.22 is actively maintained (May 2026), MSRV 1.87 < our 1.95, and its schema model is faithful to 2020-12 (verified: `TypeSet::Multiple([String, Null])`, `enum_values`, `prefix_items`, boolean schemas).
- **Keep `openapiv3 = "2"`** for 3.0 validation only. Its typed traversal is retired, but its deserializer is a strict, battle-tested 3.0 validator with existing test coverage.
- **`serde_json/preserve_order` enabled globally** (it arrives transitively via oas3's default `preserve-order` feature; we declare it explicitly). Verified probes:
  - Today's build (no preserve_order): top-level schema iteration is document order (openapiv3 `IndexMap`) but **nested inline object fields deserialize alphabetically** — observed manifest output `['alpha','zulu']` for source `zulu, alpha`.
  - With preserve_order: both crates' JSON roundtrips keep document order everywhere (verified for both dialects), so manifests become internally consistent and match what spec authors wrote.
  - Accepted consequence: nested inline object field order changes vs today's output (alphabetical → document order). Golden test files update in this change; annotation files inherit the new order.

### DR Supersessions

(none — DR-001 covers credential caching, unrelated)

### Deliberate Exclusions (recorded so they are not relitigated)

- **Webhooks (3.1/3.2 top-level `webhooks`, and callbacks).** Webhook operations are requests the API sends *to* the consumer, not callable upstream endpoints; generating `sdk.*` functions for them would mislead the LLM. The walker starts at `paths`, so webhooks fall outside the walk by construction. Verified via probe that oas3's JSON roundtrip preserves the `webhooks` key, so the exclusion is ours, not a parsing accident.
- **3.2-only constructs.** `query` method, `additionalOperations`, `querystring`, `itemSchema`, tag `kind`/`parent`. oas3 drops them during typed validation (verified via probe: `query` op and webhook do not survive the roundtrip), so a 3.2 spec generates like its 3.1 core with no special handling. Document as a limitation, not a feature.
- **OAuth2 flows, openIdConnect, mutualTLS security schemes.** Unchanged from 3.0 behavior (skipped in auth extraction).
- **oneOf/anyOf union types.** Reserved as `FieldType::OneOf`/`AnyOf` variants that never appear from valid input in this change; follow-up work implements them.

## Success Criteria (restated from brainstorm)

- 3.0.x manifests: identical **function and schema content** — names, fields, types, nullability, formats, enums, descriptions. Nested inline object field *order* changes from alphabetical to document order (the one accepted diff, decided above); golden files updated.
- Existing parser behavior cases remain green through the new public interfaces.
- New 3.1 and 3.2 fixture specs cover: type arrays with null, enum, format, allOf, additionalProperties, `$ref`, parameters, request bodies, responses, auth schemes.
- Manifests generated from the 3.1/3.2 fixtures equal their 3.0 twins modulo the 2020-12 idioms (nullable rendering identical).
- Friendly error for Swagger 2.0.

## Tests

> **Skipped.** No tests were written upfront. Follow red-green TDD as you implement —
> write a focused failing test, make it pass, move on. Aim for component-boundary
> behavioral tests (inputs, outputs, observable effects), not exhaustive coverage.

## Tests (test-write phase)

**Pre-test-write commit:** `6fa7a52d0bb296aec94f2e7a19519529d5db891d`

### Interface Files

- `src/codegen/parser.rs` — replaced with the Value-based routing surface: `SpecVersion`, `sniff_version`, `validate`, `load_spec`/`load_spec_from_file`/`load_spec_from_url`, `spec_to_manifest`, private `parse_spec_text`. Bodies are `todo!("not implemented")` stubs except loading and text parsing (ported unchanged; no extraction logic). The openapiv3 traversal is removed; the 49 pre-existing tests are re-pointed at the new API and kept as the regression suite.
- `src/codegen/normalize.rs` — `normalize_document` and `normalize_schema_node` stubs.
- `src/codegen/walker.rs` — `extract_api_config`, `extract_functions`, `extract_parameters`, `extract_schemas`, `extract_auth_config` stubs; `derive_function_name`, `camel_to_snake`, `fallback_function_name`, `resolve_server_url` ported unchanged.
- `src/codegen/manifest.rs` — reserved `FieldType::OneOf`/`FieldType::AnyOf` variants (never produced from valid input).
- `src/codegen/luau_types.rs` — reserved variants armed in `field_type_to_luau` (lower to `string`).
- `src/codegen/generate.rs` — `derive_api_name` reads `info.title` from `Value`; pipeline calls `parser::load_spec` + `spec_to_manifest`.
- `Cargo.toml` — `oas3 = "0.22"`; `serde_json` with `preserve_order`.

### Test Files

- `src/codegen/parser.rs` (`#[cfg(test)]`) — version routing, per-dialect validation, loading (YAML/JSON/missing file), the full pipeline, and ported regression behavior.
- `src/codegen/normalize.rs` (`#[cfg(test)]`) — normalization rules and walk coverage.
- `src/codegen/walker.rs` (`#[cfg(test)]`) — extraction behavior on a normalized 3.1-style fixture document.
- `tests/openapi_31_support.rs` — dialect equivalence, pre-change manifest baselines, annotation generation, and 2020-12 idiom handling end to end.
- `testdata/petstore_manifest_30.json`, `testdata/advanced_manifest_30.json` — manifests generated at `6fa7a52` for 3.0 regression comparisons.

### Behaviors Covered

#### Version routing (`parser.rs`)

- `3.0.*` routes to `V30`; `3.1.*` and `3.2.*` route to `V31Plus`
- A top-level `swagger` key errors with "Swagger 2.0 is not supported; convert to OpenAPI 3.x"
- Missing, non-string, or unrecognized `openapi` values error, naming the observed value
- Valid 3.0/3.1/3.2 documents pass dialect validation; malformed documents fail it
- 3.2 documents validate through the `V31Plus` path (oas3's `~3.1` version pin bypassed)

#### Loading (`parser.rs`)

- YAML is parsed before JSON; both dialects load from file; missing files error

#### Normalization (`normalize.rs`)

- `type: [T, "null"]` becomes `type: T` with `nullable: true`; sibling keys survive
- `["null"]`-only and empty type arrays become `{}`; single-member arrays without null gain no `nullable`
- `const: v` becomes `enum: [v]` with the `const` key removed
- Boolean schemas (`true`/`false`) become `{}`
- 3.0-style nodes and scalar-type nodes pass through unchanged
- Document walk reaches components/schemas, component parameters and request bodies, path-level and operation-level parameter schemas, requestBody and response content schemas; `$ref` values are untouched; 3.0 documents are untouched

#### Extraction (`walker.rs`)

- API config reads `servers[0].url`, `info.description`, `info.version`; relative server URLs resolve against a remote spec source (absolute URLs, local files, and missing sources keep the raw value)
- Functions are produced per operation for get/post/put/patch/delete; options/head/trace are skipped; missing `responses`, `parameters`, and `requestBody` are tolerated
- `operationId` names convert camelCase to snake_case; missing ids fall back to method+path naming with `by_<param>` segments
- Parameters: query/header/path map to locations, cookie skipped, component `$ref` parameters resolve, unresolvable refs error, type/default/enum/format come from the schema node
- Request bodies: component references resolve or error, json content preferred, first content type as fallback, `required` and `description` preserved, missing schema yields `unknown`
- Responses: first numeric 2xx with `application/json` wins, then default; `2XX` is ignored; `$ref` names and array item names unwrap; absent responses yield none
- Schemas: object schemas produce fields with description/required/enum/nullability; `allOf` merges properties and unions `required` across (and through) `$ref` members; `additionalProperties` produces map types; non-object schemas are skipped; missing components yield an empty list
- Auth: first supported inline scheme wins; http bearer -> Bearer, http basic -> Basic, apiKey (any `in`) -> ApiKey{header}; unsupported and reference schemes are skipped

#### End-to-end dialect equivalence (`tests/openapi_31_support.rs`)

- 3.1 and 3.2 petstore fixtures generate the same function set, schemas, auth, params, bodies, and responses as the 3.0 twin, field by field
- 3.1 type-array nullability renders identically to 3.0 `nullable: true` (nullable flag and field type equal); `format: uuid`/`int32`/`date-time` survive the 3.1 path
- The 3.2 fixture's `query` method and webhook produce no functions; the 3.2 manifest equals the 3.1 twin
- A 3.1 `const` field surfaces as a single-value enum; boolean-schema properties fall back to the unknown-type behavior
- Nested inline object fields keep document order (the accepted output change from `preserve_order`)
- Reserved `OneOf`/`AnyOf` variants are never produced from valid input; top-level `oneOf` schemas are skipped
- Swagger 2.0 documents fail the pipeline with the migration-guidance error
- 3.1 webhooks and 3.1/3.2 `prefixItems` are ignored
- Required nullable fields render identically through generation for all three dialects
- Complete 3.0 petstore and advanced manifests match pre-change baselines, ignoring nested-inline field order
- The 3.2 advanced manifest matches its 3.1 twin

**Review status:** approved
