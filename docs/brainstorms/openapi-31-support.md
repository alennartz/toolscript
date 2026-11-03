# OpenAPI 3.1 / 3.2 Support

## The Idea

Add OpenAPI 3.1 and 3.2 support so toolscript can generate SDKs from specs that use the JSON Schema 2020-12 data model, while keeping full 3.0.x support.

## Context and Findings

- The user initially proposed "jump to the latest version, assuming backward compatibility." OpenAPI 3.2.0 (Sep 2025) is the latest minor and is additive over 3.1. But 3.1 is a **breaking change from 3.0**: `nullable` is replaced by `type: ["string","null"]`, `exclusiveMinimum`/`exclusiveMaximum` became numbers, and schema objects are full JSON Schema 2020-12 (boolean schemas, `const`, `prefixItems`). So "latest" does not mean "replace 3.0 handling" — it means a second dialect alongside 3.0.
- **Verified failure today:** most real 3.1 specs fail to parse at all. `type: ["string","null"]` (the single most common 3.1 construct) throws `invalid type: sequence, expected a string` in `openapiv3`'s schema deserializer. A minimal 3.1 spec without type arrays parses, so failures look partial and confusing.
- **Crate situation:** `openapiv3` (current dep) supports 3.0.x only. `oas3` 0.22 (active, MSRV 1.87, our toolchain is 1.95) targets 3.1.x: models `TypeSet` type arrays (`Multiple([String, Null])`), `enum_values` as JSON values, `const_value`, `prefix_items`, boolean schemas, top-level `webhooks`, `$ref` siblings. Its version check (`~3.1` via opt-in `validate_version`) would reject 3.2.0 — routing must bypass it.
- **3.2 deltas degrade gracefully:** `oas3` parses 3.2.0 documents (verified) but silently drops the 3.2-only keys — `query` method, `additionalOperations`, `querystring`, `itemSchema`, tag `kind`/`parent`. A 3.2 spec generates like its 3.1 core.

## Key Decisions

1. **Dual parser + internal IR** (chosen over "oas3 everywhere + 3.0 shims" and "two paths, no IR").
   - Why: `oas3` does not know 3.0's `nullable` keyword and would silently drop nullability for existing users; the whole 3.0 test suite guards behaviors we would have to re-verify. Two code paths with no shared type would duplicate ~1,600 lines of extraction and drift. The IR gives one extraction path, zero 3.0 regression risk, and a seam for future schema work.
   - Considered and rejected: pre-normalizing 3.1 JSON into 3.0 shape at the `serde_json::Value` level to reuse `openapiv3`. It converts 3.1 into a 3.0 subset — information flows the wrong direction, and the JSON-level transform surface (`$ref` siblings, boolean schemas, `exclusiveMinimum` shape change) is fragile.
2. **oneOf/anyOf is follow-up work, not this scope.** The IR reserves `OneOf`/`AnyOf` variants; they lower to `String` with a doc note, as today. The IR makes the follow-up a small delta (union types in `FieldType` + Luau annotations).
3. **Webhooks are skipped in v1.** Webhook operations are requests the API sends *to* the consumer, not callable upstream endpoints; generating `sdk.*` functions for them would mislead the LLM. Skip silently; record this decision so it is not relitigated.
4. **Target the full 3.1/3.2 document surface that overlaps with today's 3.0 feature set.** No new features in this pass; the deliverable is version tolerance, not feature growth.

## Direction

- **Routing:** dispatch on the `openapi` version string — `3.0.*` → `openapiv3`, `3.1.*`/`3.2.*` → `oas3` (bypassing its `validate_version`). Swagger 2.0 (`swagger: "2.0"`) and unrecognized versions produce a clear "unsupported version" error instead of a cryptic serde failure.
- **Internal schema IR:** both parsers lower into one internal representation (manifest-level types plus normalized nullability, with reserved `OneOf`/`AnyOf` variants). Extraction, Luau annotations, and runtime consume only the IR. Live in the codegen module near `parser.rs`.
- **Normalization rules:**
  - 3.1 `type: [T, "null"]` → base type `T` with `nullable = true` → renders `T?` exactly like 3.0's `nullable: true`.
  - Boolean schemas (`true`/`false`) → permissive unknown (falls back to `String`, as unknowns do today).
  - `const` → single-value enum.
  - `prefixItems`, JSON-Schema-only keywords → ignored, consistent with today's tolerance of unsupported keywords.
- **Out of scope, documented:** webhooks (3.1/3.2), 3.2-only constructs (`query`, `additionalOperations`, `querystring`, `itemSchema`, tag nesting), OAuth2 flows, mutualTLS security scheme.

## Success Criteria

- `toolscript generate` produces manifests from 3.1.x and 3.2.x specs equivalent to the 3.0 twins of the same API.
- Petstore/advanced 3.0 manifests stay byte-identical (regression guarantee); all existing parser tests green.
- New 3.1 and 3.2 fixture specs covering: type arrays with null, enum, format, allOf, additionalProperties, `$ref`, parameters, request bodies, responses, auth schemes.
- Friendly error for Swagger 2.0.

## Open Questions

### Sharp (inputs to architecting)

- Exact IR type shape and module placement; how much of `parser.rs`'s extraction ports mechanically vs. restructures.
- `$ref` handling for oas3: reuse the existing hand-parsing of `#/components/schemas/Name` strings for consistency, or adopt `oas3`'s `resolve()` helpers.
- Fixture spec content for 3.1/3.2 (mirror petstore/advanced or add 3.1-specific coverage).

### Fog

- Real-world 3.1/3.2 specs may surface constructs we have not enumerated; tolerance policy (parse-or-fail per construct) will be judged case by case during implementation.
