# DR-002: Dual dialect validation with one normalization seam

## Status
Accepted

## Context
OpenAPI 3.1/3.2 specs use the JSON Schema 2020-12 data model (type arrays like `["string", "null"]`, `const`, boolean schemas); 3.0 uses `nullable` and numeric `exclusiveMinimum`/`exclusiveMaximum` instead. The existing `openapiv3` crate rejects most real 3.1 specs at parse time, so a second dialect had to be supported without breaking 3.0.

## Decision
Keep `openapiv3` for 3.0.x validation and add `oas3` for 3.1.x/3.2.x validation (its `~3.1` version pin is bypassed for 3.2). Each validated document is serialized back to one JSON `Value`, normalized across dialects (`type: [T, "null"]` → `type: T` + `nullable: true`, `const` → single-value enum, boolean schemas → permissive unknown), and walked by a single version-agnostic extractor into the manifest IR.

Rejected alternatives:
- **oas3 everywhere with 3.0 shims** — `oas3` does not know 3.0's `nullable` and would silently drop nullability for existing users; the whole 3.0 test suite would need re-verification.
- **Pre-normalizing 3.1 JSON down to 3.0 shape to keep only openapiv3** — it converts 3.1 into a 3.0 subset, so information flows the wrong direction, and the JSON-level transform surface (`$ref` siblings, boolean schemas, `exclusiveMinimum` shape change) is fragile.

## Consequences
- One extraction path serves all dialects; zero 3.0 regression risk from dialect handling.
- Parse errors stay dialect-precise because validation is typed per dialect; normalization sees only documents that already validated.
- New 2020-12 keywords must be added to the normalizer as they matter; unnormalized keywords fall through to existing unknown-type tolerance.
- The manifest IR stays the seam where future schema work (union types, `prefixItems`) lands.
