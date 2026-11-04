# DR-004: Document order everywhere (serde_json preserve_order)

## Status
Accepted

## Context
Adding oas3 pulled in `serde_json/preserve_order` globally (declared explicitly in `Cargo.toml`; it also arrives transitively via oas3's default features). Verified probes showed that without it, nested inline object fields deserialize alphabetically while top-level structures stay in document order — internally inconsistent generated output. With it, both dialects roundtrip in document order everywhere.

## Decision
Accept document order as the single ordering policy for generated manifests, Luau annotations, and MCP tool schema annotations (both `$defs` order and per-definition field order). The sorts that previously made MCP schema output alphabetical were dropped rather than kept as a special case.

Rejected alternative: keep alphabetical as the policy. It would require sorting at every layer, and it destroys the order the spec author wrote, which carries intent — and is what openapiv3's `IndexMap` iteration already provided at the top level before this change.

## Consequences
- An accepted, user-visible output change: nested inline object field order (and MCP tool schema schema order) changed from alphabetical to document order. Golden test files were updated in the same change.
- Existing 3.0 users' manifests differ in field order only, not content.
- Any future code must not re-introduce local sorts, or the two ordering policies will mix.
