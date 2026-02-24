# Runtime Format Validation for OpenAPI Parameters

**Date:** 2026-02-23
**Status:** Approved
**Branch:** feat/openapi-gaps

## Problem

OpenAPI specs annotate parameters with `format` strings (uuid, date-time, email, etc.) and `enum` constraints. Today, the Rust layer extracts these at codegen time but only uses them as documentation comments in Luau annotations. Invalid values pass straight through to the remote API, which returns opaque 400/422 errors.

## Decision

Add strict runtime validation of parameter formats and enum values in the Rust layer, before any HTTP request is made. Return clear, actionable error messages to the Luau caller.

## Scope

- **Parameters only:** path, query, and header parameters. Request body validation is out of scope (follow-up via `jsonschema` crate).
- **Strict mode:** invalid values produce errors, no warnings-only mode.
- **Unknown formats:** silently passed through (the API validates its own custom formats).

## Data Flow

```
Luau calls sdk.get_pet("not-a-uuid")
  → registry.rs param loop extracts value
  → calls validate::validate_param_value(&func_name, &param, &str_value)
  → validate.rs checks param.format == Some("uuid"), runs uuid check
  → returns Err("parameter 'pet_id' for 'get_pet': expected uuid format, got 'not-a-uuid'")
  → registry returns mlua::Error::external(...)
  → Luau gets a clear error, no HTTP request made
```

## Changes

### 1. Manifest: Add `format` to `ParamDef`

In `src/codegen/manifest.rs`, add to `ParamDef`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub format: Option<String>,
```

### 2. Parser: Extract format from parameter schemas

In `src/codegen/parser.rs`, `extract_param_type_info()` already has the schema. Extract the format string using the existing `extract_format()` helper and include it in the return tuple. Wire it into the `ParamDef` construction at line 243.

### 3. New module: `src/runtime/validate.rs`

Public API:

```rust
pub fn validate_param_value(
    func_name: &str,
    param: &ParamDef,
    value: &str,
) -> Result<(), mlua::Error>
```

Checks enum values first, then format. Returns `Ok(())` or an error with message:
`parameter '{name}' for '{func}': {detail}`

### 4. Format validators

| Format | Method | Dependency |
|--------|--------|------------|
| `uuid` | Hex pattern `8-4-4-4-12` check | std |
| `date-time` | RFC 3339 shape parse | std |
| `date` | `YYYY-MM-DD` parse | std |
| `email` | `@` with non-empty local/domain, domain has `.` | std |
| `uri` / `url` | `url::Url::parse()` | `url` (already in Cargo.toml) |
| `ipv4` | `Ipv4Addr::from_str()` | std |
| `ipv6` | `Ipv6Addr::from_str()` | std |
| `int32` | Parse i64, check `[-2^31, 2^31-1]` | std |
| `int64` | Parse i64 succeeds | std |
| `hostname` | Label rules: 1-63 chars, alnum+hyphen, total ≤253 | std |
| unknown | `Ok(())` | — |

### 5. Enum validation

```rust
if let Some(ref allowed) = param.enum_values {
    if !allowed.iter().any(|v| v == value) {
        return Err(...);  // "expected one of [...], got '...'"
    }
}
```

### 6. Registry integration

In `src/runtime/registry.rs`, after the required-param check (line 103) and before string coercion (line 110), call `validate::validate_param_value()`. On error, return immediately.

## Testing

**Unit tests** (`validate.rs`, `#[cfg(test)]`):
- One test per format: valid passes, invalid returns expected error
- Enum: match passes, non-match lists allowed values
- Unknown format: always passes
- Edge cases: empty strings, int32/int64 boundaries, mixed-case UUIDs

**Integration test** (in `tests/` directory):
- Load `advanced.yaml` test spec
- Call function with invalid UUID param → assert format error
- Call with valid UUID → assert validation passes (connection error is fine)

## Dependencies

No new crates. `url` is already in `Cargo.toml`. Everything else uses `std`.

## Future Work

- Request body validation via `jsonschema` crate (schema-driven, recursive)
- Custom validator registration for API-specific formats
