# DR-003: Webhooks and callbacks excluded from SDK generation

## Status
Accepted

## Context
OpenAPI 3.1/3.2 documents can declare top-level `webhooks` (and per-operation `callbacks`) — operations the API sends *to* the consumer, not endpoints the consumer can call. The oas3 JSON roundtrip preserves these keys, so they are fully visible to the generator; the exclusion is deliberate, not a parsing accident.

## Decision
The extractor starts its walk at `paths`, so webhook and callback operations fall outside the walk by construction. No SDK functions are generated for them.

Rejected alternative: generate webhook operations as `sdk.*` functions. Webhook operations are requests the API sends to the consumer, so a callable function would mislead the LLM into calling an endpoint that does not exist upstream.

## Consequences
- 3.1/3.2 specs with webhooks generate like their paths-only core; users see no functions for webhook operations and no error. Documented as a limitation in the README.
- If bidirectional support ever lands, it will be a new surface (a way to receive), not this one.
