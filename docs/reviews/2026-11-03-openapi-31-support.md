# Review: OpenAPI 3.1 / 3.2 Support

**Plan:** `docs/plans/2026-11-03-openapi-31-support.md`
**Diff range:** `6fa7a52..6242edc` (test immutability checked `bf8a9fa..6242edc`)
**Date:** 2026-11-03

## Summary

The plan was implemented faithfully across all nine steps: every interface matches the plan's Rust signatures, all behaviors in the Tests section are covered, and nothing in the diff traces to work outside the plan. Test immutability holds — test modules in `parser.rs`/`normalize.rs` are byte-identical during implementation, `walker.rs`/`luau_types.rs` test additions are additive only, and the disclosed fixture-defect fixes match the plan's Step 8 note exactly. The code-correctness pass found five warnings in the new normalization and walker code, all triggered by spec shapes the fixtures do not exercise (components-less documents, `allOf` with sibling `properties`, empty object schemas, 3.2 `querystring` parameters, cyclic `allOf`). Repo integrity verified: working tree clean, the pre-existing `feature/upstream-mcp` stash intact and unrelated, and no dangling commits from this phase.

## Findings

### 1. `normalize_document` skips the `paths` walk when `components` is absent

- **Category:** plan deviation
- **Severity:** warning
- **Location:** `src/codegen/normalize.rs:23-25`
- **Status:** resolved
- **Fix:** replace the early `return` with an `if let Some(components)` block so the `paths` walk always runs.

Plan Step 2 and the Interfaces section specify the document walk over path-level and operation-level parameter schemas and request-body/response content schemas, unconditionally. The function returns early when the document has no `components` object, so those locations go unnormalized. The plan's Behaviors Covered list has no components precondition, and components-less specs (inline-everything APIs) are a common shape. Reproduced: `type: [integer, string]` on a path parameter becomes `ParamType::Integer` without `components` and `ParamType::String` with an empty `components: {schemas: {}}` — the same spec types differently depending on an unrelated key. Current output is partly masked because `luau_types` independently tolerates 2020-12 shapes, but the planned normalization seam is bypassed.

### 2. Schemas with both `allOf` and sibling `properties` lose their `allOf` members

- **Category:** code correctness
- **Severity:** warning
- **Location:** `src/codegen/walker.rs:335-384`
- **Status:** resolved
- **Fix:** delete the separate `properties` and `allOf` branches; call `collect_object_properties` on the root schema when either key is present (it already merges both).

`extract_schema_def` checks `properties` first and returns, ignoring `allOf`. For `Derived: {allOf: [{$ref: Base}], properties: {name}}` the probe produced `Derived fields=[name]` — every field from `Base` is silently dropped. In JSON Schema 2020-12 the combination is valid and common; the `oas3` round-trip preserves both keys, so 3.1 specs hit this. In 3.0, `openapiv3` drops the sibling properties, so 3.0 output matches the old behavior and the baselines do not catch it.

### 3. Object schemas with no `properties` no longer produce a `SchemaDef` (regression)

- **Category:** code correctness
- **Severity:** warning
- **Location:** `src/codegen/walker.rs:335-384`
- **Status:** resolved
- **Fix:** gate the rewrite from finding 2 on `type == "object"` so empty objects produce a `SchemaDef` with no fields.

The old parser produced a `SchemaDef` with no fields for every object schema (`SchemaKind::Type(Object)`). The new walker requires a `properties` key, and both `openapiv3` and `oas3` omit empty `properties` when serializing — so `Empty: {type: object}` and `Dict: {type: object, additionalProperties: ...}` now vanish from `components.schemas`. Probe-confirmed for 3.0. Fields that `$ref` these schemas still emit `FieldType::Object { schema: "Empty" }`, so the generated annotations name a Luau type that nothing defines. This also violates the plan's success criterion of identical 3.0 function and schema content.

### 4. Valid 3.2 documents using `in: querystring` or `style: cookie` fail validation; README overclaims

- **Category:** code correctness
- **Severity:** warning
- **Location:** `src/codegen/parser.rs:130-137`
- **Status:** resolved
- **Fix:** correct `README.md:432` to say such specs fail validation, or strip `querystring` parameters and 3.2-only `style` values before V31Plus typed validation.

The `oas3` 0.22 enums `ParameterIn` and `ParameterStyle` lack these 3.2 values, so deserialization fails with "data did not match any variant of untagged enum ObjectOrReference" — an error that names no field — and the whole spec is rejected. `README.md:432` claims specs using `querystring` "generate from their 3.1 core; those constructs are ignored." They do not; the plan's deliberate exclusion was that the constructs degrade silently, but `oas3`'s typed model rejects the document outright. The README contradicts actual behavior.

### 5. A cyclic `allOf` through `$ref` overflows the stack and aborts the process

- **Category:** code correctness
- **Severity:** warning
- **Location:** `src/codegen/walker.rs:387-436`
- **Status:** resolved
- **Fix:** pass a `&mut HashSet<&str>` of visited schema names into `collect_object_properties` and return early on a repeat.

`A: {allOf: [{$ref: '#/components/schemas/A'}]}` (or an A→B→A cycle) recurses without limit. The probe hit `fatal runtime error: stack overflow, aborting` — a hard process abort, not an `Err`, so `generate` cannot report it as a spec problem. Pre-existing behavior carried over from the old parser, but the rewrite was the opportunity to bound it, and new code paths reproduce it.

### 6. MCP `$defs` extraction silently drops its alphabetical ordering

- **Category:** plan deviation
- **Severity:** nit
- **Location:** `src/codegen/luau_types.rs:219-249`
- **Status:** open
- **Fix:** restore the two sorts, or record the MCP schema-order change next to the accepted inline-order consequence in the plan and README.

Plan Step 3 authorizes document order only in `object_field_type`, and the Technology Choices accepted-consequence note covers only nested inline object field order. Removing the defs sort and field sort in `extract_schema_defs` also changes MCP tool schema annotation order (both def order and field order). Same class of change and consistent with the global `preserve_order` decision, but outside the disclosed scope.

### 7. A chained parameter `$ref` drops the parameter silently

- **Category:** code correctness
- **Severity:** nit
- **Location:** `src/codegen/walker.rs:128` and `155-171`
- **Status:** resolved
- **Fix:** in `resolve_parameter`, return an error when the resolved value still contains `$ref`.

When `components.parameters.X` is itself a `$ref`, the resolver returns the `{"$ref": ...}` object instead of erroring. That object has no `in` key, so `_ => continue` treats it like a skipped cookie parameter. The old code bailed with "Could not resolve parameter ref"; the new code silently drops a possibly required path parameter.

## No Issues

- **Plan adherence, Steps 1, 4, 5, 6, 7, 8, 9:** no deviations found. Version routing, loading, auth extraction, parameter extraction, operation/body/response extraction, schema extraction, the pipeline connection, and the README documentation all match the plan, including ported behavior verified against the pre-change source (server URL resolution, method order, first-2xx response selection, allOf required-union, skip rules).
- **Reverse check:** all diff content traces to a plan step; nothing unplanned.
- **Test immutability:** upheld (see Summary). `advanced_manifest_30.json` untouched; `petstore_manifest_30.json` regenerated exactly as disclosed with user approval.
- **Verification at HEAD:** `cargo test` — 466 passed (9 suites); `cargo fmt --check` and `cargo clippy -- -D warnings` clean.
- **Code correctness, clean areas:** reserved `FieldType::OneOf`/`AnyOf` cannot leak into output (planned reserved variants for follow-up); recursion in `normalize.rs` and `luau_types.rs` is `$ref`-blind and depth-capped by the serde parsers; first-2xx selection, document order, body/parameter ref error paths, auth selection, name derivation, Swagger 2.0 rejection, and `generate.rs` are clean.
