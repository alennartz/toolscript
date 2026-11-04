# DR-005: Reserved OneOf/AnyOf manifest variants

## Status
Accepted

## Context
oneOf/anyOf union types are not implemented in the generator, but the manifest IR gained `FieldType::OneOf` and `FieldType::AnyOf` variants that lower to `string` and are never produced from valid input today. The choice was whether to add the variants now or wait for the union feature.

## Decision
Reserve the variants now rather than adding them with the follow-up. The manifest IR is the seam where union support lands, so adding the variants up front makes the follow-up a small delta (union extraction in the walker plus Luau rendering) instead of an IR change rippling through the serde shape and every consumer.

Rejected alternative: add the variants only when unions are implemented. Without them, a future reader could mistake the string-lowering for dead code and delete it, re-opening the IR change.

## Consequences
- Two enum variants exist that no valid input produces; they lower to `string` like today's unknown types.
- When the follow-up implements unions, these variants get their real payloads and rendering.
- Documented in `manifest.rs` doc comments.
