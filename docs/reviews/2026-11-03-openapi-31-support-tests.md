# Test Review: OpenAPI 3.1 / 3.2 Support

**Plan:** `docs/plans/2026-11-03-openapi-31-support.md`  
**Brainstorm:** `docs/brainstorms/openapi-31-support.md`  
**Date:** 2026-11-03

## Summary

The reviewed tests cover dialect routing, schema normalization, extraction, and generation. The review closed coverage and architecture gaps, corrected invalid fixtures, and moved private-helper tests to component boundaries. The tests remain red against the planned stubs.

## Findings

### 1. Referenced component schemas and request bodies

- **Category:** missing coverage
- **Severity:** critical
- **Location:** `src/codegen/normalize.rs:216-235`, `src/codegen/walker.rs:716-738`, `tests/openapi_31_support.rs:533-560`
- **Status:** resolved

The architecture omitted normalization of component parameters and request bodies. It also omitted request-body reference resolution, which the 3.0 parser supported. The user approved the architecture update and tests for normalization, resolved references, and unresolved references.

### 2. End-to-end dialect and regression coverage

- **Category:** missing coverage
- **Severity:** critical
- **Location:** `tests/openapi_31_support.rs:493-530`, `tests/openapi_31_support.rs:562-645`
- **Status:** resolved

The tests did not cover 3.1 webhooks, ignored `prefixItems`, required nullable Luau output, or complete pre-change 3.0 manifest content. The 3.2 advanced fixture was unused. With user approval, tests cover those behaviors. The 3.0 comparisons use manifests generated at `6fa7a52` and ignore only nested inline field order.

### 3. Incorrect walker and boolean schema fixtures

- **Category:** missing coverage
- **Severity:** critical
- **Location:** `src/codegen/walker.rs:294-316`, `src/codegen/walker.rs:690-700`, `src/codegen/walker.rs:772-924`, `tests/openapi_31_support.rs:339-368`
- **Status:** resolved

The boolean-schema fixture contained an array instead of a boolean. Walker tests expected an absent authentication scheme, omitted an enum value, supplied unnormalized schema data, and expected a relative URL result from an absolute URL. Corrected inputs and assertions now exercise the documented contracts.

### 4. Private helper tests

- **Category:** wrong abstraction
- **Severity:** warning
- **Location:** `src/codegen/walker.rs:576-648`
- **Status:** resolved

Name and server URL cases called private helpers. With user approval, the tests now exercise `extract_functions` and `extract_api_config`. They retain the behavior cases without requiring private helper functions or a fixed test count.

### 5. Authentication selection ambiguity

- **Category:** over-specified
- **Severity:** warning
- **Location:** `src/codegen/walker.rs:58-65`, `src/codegen/walker.rs:752-766`
- **Status:** resolved

The first-item wording conflicted with the previous parser, which skips unsupported and referenced schemes. The user approved the existing behavior: select the first supported inline scheme. The interface and a mixed-scheme test now state that contract.

### 6. Response, body, and unsupported method paths

- **Category:** missing coverage
- **Severity:** warning
- **Location:** `src/codegen/walker.rs:702-750`
- **Status:** resolved

Tests did not establish default-response fallback, JSON content preference over another media type, or exclusion of all unsupported methods. With user approval, boundary tests cover these paths. The user confirmed that wildcard `2XX` responses remain ignored, as in the previous parser.

### 7. Missing-version error wording

- **Category:** over-specified
- **Severity:** warning
- **Location:** `src/codegen/parser.rs:147-151`
- **Status:** resolved

The test required an error phrase not specified by the interface. It now requires an error without prescribing wording.

### 8. Reserved union representation

- **Category:** over-specified
- **Severity:** nit
- **Location:** `tests/openapi_31_support.rs:402-456`
- **Status:** resolved

The union test constructed reserved variants with a specific internal payload. The architecture reserves the variants but does not require that payload. The test now checks only that supported input does not produce them.

## No Issues

The remaining new tests use materialized component interfaces and observable outputs. They do not depend on external network services, wall-clock time, random values, or filesystem iteration order. The pre-existing private helper cases now use component interfaces.
