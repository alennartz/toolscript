# Format Validation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Validate OpenAPI parameter formats and enum values at runtime before HTTP requests, returning clear errors to Luau callers.

**Architecture:** New `src/runtime/validate.rs` module with format-specific validators. Parser extracts format strings into `ParamDef`. Registry calls validation after required-check, before string coercion.

**Tech Stack:** Rust std (net, str parsing), `url` crate (already in Cargo.toml), `mlua` for error types.

---

### Task 1: Add `format` field to `ParamDef`

**Files:**
- Modify: `src/codegen/manifest.rs:60-68`

**Step 1: Add the field**

In `ParamDef` struct, add after `enum_values`:

```rust
pub struct ParamDef {
    pub name: String,
    pub location: ParamLocation,
    pub param_type: ParamType,
    pub required: bool,
    pub description: Option<String>,
    pub default: Option<serde_json::Value>,
    pub enum_values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}
```

**Step 2: Fix all compilation errors**

Every `ParamDef { ... }` literal in the codebase needs `format: None` added. Search for all occurrences:

- `src/runtime/registry.rs` — test helpers (lines 250-258, 272-289, 370-378, 478-495, 551-558, 616-632)
- `src/runtime/executor.rs` — line 208
- `src/codegen/parser.rs` — line 243-251
- `src/codegen/manifest.rs` — test helpers (lines 162-174, 175-183, 370-378)

For all except `parser.rs:243`, add `format: None`. For `parser.rs:243`, wire in the extracted format (Task 2).

**Step 3: Run tests**

Run: `cargo test`
Expected: All existing tests pass (format defaults to None everywhere).

**Step 4: Commit**

```
feat: add format field to ParamDef manifest struct
```

---

### Task 2: Extract format from parameter schemas in the parser

**Files:**
- Modify: `src/codegen/parser.rs:279-294` (`extract_param_type_info`)
- Modify: `src/codegen/parser.rs:241-251` (call site)

**Step 1: Write the failing test**

In `src/codegen/parser.rs` test module, add:

```rust
#[test]
fn test_param_format_extraction() {
    let spec = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
    let manifest = spec_to_manifest(&spec, "advanced").unwrap();

    // The getResource function has path param `id` with format: uuid
    let get_resource = manifest
        .functions
        .iter()
        .find(|f| f.name == "get_resource")
        .expect("get_resource function missing");

    let id_param = get_resource
        .parameters
        .iter()
        .find(|p| p.name == "id")
        .expect("id param missing");

    assert_eq!(id_param.format.as_deref(), Some("uuid"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_param_format_extraction -- --nocapture`
Expected: FAIL — `id_param.format` is `None` because `extract_param_type_info` doesn't return format yet.

**Step 3: Implement format extraction**

Change `extract_param_type_info` return type to include format:

```rust
fn extract_param_type_info(
    format: &ParameterSchemaOrContent,
) -> (ParamType, Option<serde_json::Value>, Option<Vec<String>>, Option<String>) {
    match format {
        ParameterSchemaOrContent::Schema(schema_ref) => {
            if let ReferenceOr::Item(schema) = schema_ref {
                let param_type = schema_type_to_param_type(&schema.schema_kind);
                let default_val = schema.schema_data.default.clone();
                let enum_values = extract_enum_values(&schema.schema_kind);
                let fmt = extract_format(&schema.schema_kind);
                (param_type, default_val, enum_values, fmt)
            } else {
                (ParamType::String, None, None, None)
            }
        }
        ParameterSchemaOrContent::Content(_) => (ParamType::String, None, None, None),
    }
}
```

Update the call site at line 241:

```rust
let (param_type, default_val, enum_values, format) = extract_param_type_info(&data.format);

result.push(ParamDef {
    name: data.name.clone(),
    location,
    param_type,
    required: data.required,
    description: data.description.clone(),
    default: default_val,
    enum_values,
    format,
});
```

**Step 4: Run tests**

Run: `cargo test`
Expected: All tests pass including the new one.

**Step 5: Commit**

```
feat: extract format from parameter schemas in parser
```

---

### Task 3: Create `validate.rs` with enum validation

**Files:**
- Create: `src/runtime/validate.rs`
- Modify: `src/runtime/mod.rs`

**Step 1: Write the failing test**

Create `src/runtime/validate.rs` with just the test module and public function signature:

```rust
use crate::codegen::manifest::ParamDef;

/// Validate a parameter value against its enum constraints and format.
/// Returns `Ok(())` if valid, or an `mlua::Error` with a descriptive message.
pub fn validate_param_value(
    func_name: &str,
    param: &ParamDef,
    value: &str,
) -> Result<(), mlua::Error> {
    todo!()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::codegen::manifest::{ParamLocation, ParamType};

    fn make_param(name: &str, enum_values: Option<Vec<String>>, format: Option<String>) -> ParamDef {
        ParamDef {
            name: name.to_string(),
            location: ParamLocation::Query,
            param_type: ParamType::String,
            required: true,
            description: None,
            default: None,
            enum_values,
            format,
        }
    }

    #[test]
    fn test_enum_valid_value() {
        let param = make_param("status", Some(vec!["active".into(), "inactive".into()]), None);
        assert!(validate_param_value("list_users", &param, "active").is_ok());
    }

    #[test]
    fn test_enum_invalid_value() {
        let param = make_param("status", Some(vec!["active".into(), "inactive".into()]), None);
        let err = validate_param_value("list_users", &param, "deleted").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("list_users"), "should contain func name: {msg}");
        assert!(msg.contains("status"), "should contain param name: {msg}");
        assert!(msg.contains("active"), "should list allowed values: {msg}");
        assert!(msg.contains("deleted"), "should show actual value: {msg}");
    }

    #[test]
    fn test_no_enum_no_format_passes() {
        let param = make_param("name", None, None);
        assert!(validate_param_value("get_user", &param, "anything").is_ok());
    }
}
```

Add `pub mod validate;` to `src/runtime/mod.rs`.

**Step 2: Run test to verify it fails**

Run: `cargo test validate -- --nocapture`
Expected: FAIL — `todo!()` panics.

**Step 3: Implement enum validation**

Replace the `todo!()` body:

```rust
pub fn validate_param_value(
    func_name: &str,
    param: &ParamDef,
    value: &str,
) -> Result<(), mlua::Error> {
    // Check enum constraint first
    if let Some(ref allowed) = param.enum_values {
        if !allowed.iter().any(|v| v == value) {
            return Err(mlua::Error::external(anyhow::anyhow!(
                "parameter '{}' for '{}': expected one of [{}], got '{}'",
                param.name,
                func_name,
                allowed.join(", "),
                value,
            )));
        }
    }

    // Check format constraint
    if let Some(ref fmt) = param.format {
        validate_format(func_name, &param.name, fmt, value)?;
    }

    Ok(())
}

fn validate_format(
    func_name: &str,
    param_name: &str,
    format: &str,
    value: &str,
) -> Result<(), mlua::Error> {
    let result = match format {
        _ => return Ok(()), // Unknown formats pass through (temporary)
    };
}
```

**Step 4: Run tests**

Run: `cargo test validate`
Expected: All 3 tests pass.

**Step 5: Commit**

```
feat: add validate module with enum validation
```

---

### Task 4: Add format validators (uuid, date-time, date)

**Files:**
- Modify: `src/runtime/validate.rs`

**Step 1: Write the failing tests**

Add to the test module:

```rust
#[test]
fn test_uuid_valid() {
    let param = make_param("id", None, Some("uuid".into()));
    assert!(validate_param_value("get_user", &param, "550e8400-e29b-41d4-a716-446655440000").is_ok());
}

#[test]
fn test_uuid_valid_uppercase() {
    let param = make_param("id", None, Some("uuid".into()));
    assert!(validate_param_value("get_user", &param, "550E8400-E29B-41D4-A716-446655440000").is_ok());
}

#[test]
fn test_uuid_invalid() {
    let param = make_param("id", None, Some("uuid".into()));
    let err = validate_param_value("get_user", &param, "not-a-uuid").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("uuid"), "should mention format: {msg}");
    assert!(msg.contains("not-a-uuid"), "should show value: {msg}");
}

#[test]
fn test_uuid_invalid_short() {
    let param = make_param("id", None, Some("uuid".into()));
    assert!(validate_param_value("get_user", &param, "550e8400-e29b-41d4").is_err());
}

#[test]
fn test_datetime_valid() {
    let param = make_param("since", None, Some("date-time".into()));
    assert!(validate_param_value("list", &param, "2024-01-15T10:30:00Z").is_ok());
}

#[test]
fn test_datetime_valid_offset() {
    let param = make_param("since", None, Some("date-time".into()));
    assert!(validate_param_value("list", &param, "2024-01-15T10:30:00+05:30").is_ok());
}

#[test]
fn test_datetime_valid_fractional() {
    let param = make_param("since", None, Some("date-time".into()));
    assert!(validate_param_value("list", &param, "2024-01-15T10:30:00.123Z").is_ok());
}

#[test]
fn test_datetime_invalid() {
    let param = make_param("since", None, Some("date-time".into()));
    assert!(validate_param_value("list", &param, "2024-01-15").is_err());
}

#[test]
fn test_datetime_invalid_garbage() {
    let param = make_param("since", None, Some("date-time".into()));
    assert!(validate_param_value("list", &param, "not-a-date").is_err());
}

#[test]
fn test_date_valid() {
    let param = make_param("dob", None, Some("date".into()));
    assert!(validate_param_value("create", &param, "2024-01-15").is_ok());
}

#[test]
fn test_date_invalid() {
    let param = make_param("dob", None, Some("date".into()));
    assert!(validate_param_value("create", &param, "01-15-2024").is_err());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test validate`
Expected: New tests FAIL — format validators not implemented yet (unknown formats pass through).

**Step 3: Implement uuid, date-time, date validators**

Add to `validate.rs`:

```rust
fn validate_format(
    func_name: &str,
    param_name: &str,
    format: &str,
    value: &str,
) -> Result<(), mlua::Error> {
    let valid = match format {
        "uuid" => is_valid_uuid(value),
        "date-time" => is_valid_datetime(value),
        "date" => is_valid_date(value),
        _ => return Ok(()), // Unknown formats pass through
    };

    if valid {
        Ok(())
    } else {
        Err(mlua::Error::external(anyhow::anyhow!(
            "parameter '{}' for '{}': expected {} format, got '{}'",
            param_name,
            func_name,
            format,
            value,
        )))
    }
}

fn is_valid_uuid(value: &str) -> bool {
    // UUID: 8-4-4-4-12 hex digits
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lens = [8, 4, 4, 4, 12];
    parts
        .iter()
        .zip(expected_lens.iter())
        .all(|(part, &len)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

fn is_valid_datetime(value: &str) -> bool {
    // RFC 3339: YYYY-MM-DDTHH:MM:SS[.frac](Z|+HH:MM|-HH:MM)
    let Some((date_part, rest)) = value.split_once('T') else {
        return false;
    };
    if !is_valid_date(date_part) {
        return false;
    }
    // Split off timezone: Z, +HH:MM, or -HH:MM
    let (time_part, tz) = if let Some(pos) = rest.rfind('Z') {
        (&rest[..pos], &rest[pos..])
    } else if let Some(pos) = rest.rfind('+') {
        (&rest[..pos], &rest[pos..])
    } else if let Some(pos) = rest[1..].rfind('-') {
        // skip first char to avoid matching negative sign
        (&rest[..pos + 1], &rest[pos + 1..])
    } else {
        return false;
    };
    // Validate time: HH:MM:SS[.frac]
    let time_core = time_part.split('.').next().unwrap_or("");
    let time_parts: Vec<&str> = time_core.split(':').collect();
    if time_parts.len() != 3 {
        return false;
    }
    if !time_parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    // Validate timezone
    match tz {
        "Z" => true,
        _ if tz.starts_with('+') || tz.starts_with('-') => {
            let tz_parts: Vec<&str> = tz[1..].split(':').collect();
            tz_parts.len() == 2
                && tz_parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit()))
        }
        _ => false,
    }
}

fn is_valid_date(value: &str) -> bool {
    // YYYY-MM-DD
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == 3
        && parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
}
```

**Step 4: Run tests**

Run: `cargo test validate`
Expected: All tests pass.

**Step 5: Commit**

```
feat: add uuid, date-time, date format validators
```

---

### Task 5: Add format validators (email, uri, ip, hostname)

**Files:**
- Modify: `src/runtime/validate.rs`

**Step 1: Write the failing tests**

```rust
#[test]
fn test_email_valid() {
    let param = make_param("email", None, Some("email".into()));
    assert!(validate_param_value("create", &param, "user@example.com").is_ok());
}

#[test]
fn test_email_invalid_no_at() {
    let param = make_param("email", None, Some("email".into()));
    assert!(validate_param_value("create", &param, "userexample.com").is_err());
}

#[test]
fn test_email_invalid_no_domain_dot() {
    let param = make_param("email", None, Some("email".into()));
    assert!(validate_param_value("create", &param, "user@localhost").is_err());
}

#[test]
fn test_uri_valid() {
    let param = make_param("website", None, Some("uri".into()));
    assert!(validate_param_value("create", &param, "https://example.com/path").is_ok());
}

#[test]
fn test_url_valid() {
    let param = make_param("website", None, Some("url".into()));
    assert!(validate_param_value("create", &param, "https://example.com").is_ok());
}

#[test]
fn test_uri_invalid() {
    let param = make_param("website", None, Some("uri".into()));
    assert!(validate_param_value("create", &param, "not a url").is_err());
}

#[test]
fn test_ipv4_valid() {
    let param = make_param("ip", None, Some("ipv4".into()));
    assert!(validate_param_value("create", &param, "192.168.1.1").is_ok());
}

#[test]
fn test_ipv4_invalid() {
    let param = make_param("ip", None, Some("ipv4".into()));
    assert!(validate_param_value("create", &param, "999.999.999.999").is_err());
}

#[test]
fn test_ipv6_valid() {
    let param = make_param("ip", None, Some("ipv6".into()));
    assert!(validate_param_value("create", &param, "::1").is_ok());
}

#[test]
fn test_ipv6_invalid() {
    let param = make_param("ip", None, Some("ipv6".into()));
    assert!(validate_param_value("create", &param, "not-ipv6").is_err());
}

#[test]
fn test_hostname_valid() {
    let param = make_param("host", None, Some("hostname".into()));
    assert!(validate_param_value("create", &param, "api.example.com").is_ok());
}

#[test]
fn test_hostname_invalid_underscore() {
    let param = make_param("host", None, Some("hostname".into()));
    assert!(validate_param_value("create", &param, "bad_host.com").is_err());
}

#[test]
fn test_hostname_invalid_long_label() {
    let param = make_param("host", None, Some("hostname".into()));
    let long_label = "a".repeat(64);
    assert!(validate_param_value("create", &param, &format!("{long_label}.com")).is_err());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test validate`
Expected: New tests FAIL.

**Step 3: Implement email, uri, ip, hostname validators**

Add the match arms and helper functions:

```rust
// In validate_format match:
"email" => is_valid_email(value),
"uri" | "url" => is_valid_uri(value),
"ipv4" => is_valid_ipv4(value),
"ipv6" => is_valid_ipv6(value),
"hostname" => is_valid_hostname(value),

// Helpers:
fn is_valid_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && !domain.is_empty() && domain.contains('.')
}

fn is_valid_uri(value: &str) -> bool {
    url::Url::parse(value).is_ok()
}

fn is_valid_ipv4(value: &str) -> bool {
    value.parse::<std::net::Ipv4Addr>().is_ok()
}

fn is_valid_ipv6(value: &str) -> bool {
    value.parse::<std::net::Ipv6Addr>().is_ok()
}

fn is_valid_hostname(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 {
        return false;
    }
    value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}
```

**Step 4: Run tests**

Run: `cargo test validate`
Expected: All tests pass.

**Step 5: Commit**

```
feat: add email, uri, ip, hostname format validators
```

---

### Task 6: Add format validators (int32, int64) and unknown format pass-through test

**Files:**
- Modify: `src/runtime/validate.rs`

**Step 1: Write the failing tests**

```rust
#[test]
fn test_int32_valid() {
    let param = make_param("code", None, Some("int32".into()));
    assert!(validate_param_value("get", &param, "42").is_ok());
}

#[test]
fn test_int32_valid_negative() {
    let param = make_param("code", None, Some("int32".into()));
    assert!(validate_param_value("get", &param, "-2147483648").is_ok());
}

#[test]
fn test_int32_valid_max() {
    let param = make_param("code", None, Some("int32".into()));
    assert!(validate_param_value("get", &param, "2147483647").is_ok());
}

#[test]
fn test_int32_overflow() {
    let param = make_param("code", None, Some("int32".into()));
    assert!(validate_param_value("get", &param, "2147483648").is_err());
}

#[test]
fn test_int32_underflow() {
    let param = make_param("code", None, Some("int32".into()));
    assert!(validate_param_value("get", &param, "-2147483649").is_err());
}

#[test]
fn test_int32_not_a_number() {
    let param = make_param("code", None, Some("int32".into()));
    assert!(validate_param_value("get", &param, "abc").is_err());
}

#[test]
fn test_int64_valid() {
    let param = make_param("big", None, Some("int64".into()));
    assert!(validate_param_value("get", &param, "9223372036854775807").is_ok());
}

#[test]
fn test_int64_invalid() {
    let param = make_param("big", None, Some("int64".into()));
    assert!(validate_param_value("get", &param, "not-a-number").is_err());
}

#[test]
fn test_unknown_format_passes() {
    let param = make_param("custom", None, Some("custom-id".into()));
    assert!(validate_param_value("get", &param, "literally anything").is_ok());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test validate`
Expected: int32/int64 tests FAIL; unknown format test already passes.

**Step 3: Implement int32, int64 validators**

Add match arms:

```rust
"int32" => is_valid_int32(value),
"int64" => is_valid_int64(value),
```

Add helpers:

```rust
fn is_valid_int32(value: &str) -> bool {
    value.parse::<i64>().is_ok_and(|n| n >= i64::from(i32::MIN) && n <= i64::from(i32::MAX))
}

fn is_valid_int64(value: &str) -> bool {
    value.parse::<i64>().is_ok()
}
```

**Step 4: Run tests**

Run: `cargo test validate`
Expected: All tests pass.

**Step 5: Commit**

```
feat: add int32, int64 format validators
```

---

### Task 7: Wire validation into registry

**Files:**
- Modify: `src/runtime/registry.rs:6` (imports), `src/runtime/registry.rs:106-116` (validation call)

**Step 1: Write the failing test**

Add to `src/runtime/registry.rs` test module:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_enum_validation_rejects_invalid() {
    let manifest = Manifest {
        apis: vec![ApiConfig {
            name: "testapi".to_string(),
            base_url: "https://api.example.com".to_string(),
            description: None,
            version: None,
            auth: None,
        }],
        functions: vec![FunctionDef {
            name: "list_items".to_string(),
            api: "testapi".to_string(),
            tag: None,
            method: HttpMethod::Get,
            path: "/items".to_string(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: vec![ParamDef {
                name: "status".to_string(),
                location: ParamLocation::Query,
                param_type: ParamType::String,
                required: true,
                description: None,
                default: None,
                enum_values: Some(vec!["active".into(), "inactive".into()]),
                format: None,
            }],
            request_body: None,
            response_schema: None,
        }],
        schemas: vec![],
    };

    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    let handler = Arc::new(HttpHandler::mock(|_method, _url, _query, _body| {
        panic!("HTTP request should not be made for invalid enum value");
    }));
    let creds = Arc::new(AuthCredentialsMap::new());
    let counter = Arc::new(AtomicUsize::new(0));

    register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

    let result = sb.eval::<Value>(r#"sdk.list_items("deleted")"#);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("expected one of"), "error was: {err}");
    assert!(err.contains("deleted"), "error was: {err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_format_validation_rejects_invalid_uuid() {
    let manifest = Manifest {
        apis: vec![ApiConfig {
            name: "testapi".to_string(),
            base_url: "https://api.example.com".to_string(),
            description: None,
            version: None,
            auth: None,
        }],
        functions: vec![FunctionDef {
            name: "get_item".to_string(),
            api: "testapi".to_string(),
            tag: None,
            method: HttpMethod::Get,
            path: "/items/{id}".to_string(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: vec![ParamDef {
                name: "id".to_string(),
                location: ParamLocation::Path,
                param_type: ParamType::String,
                required: true,
                description: None,
                default: None,
                enum_values: None,
                format: Some("uuid".into()),
            }],
            request_body: None,
            response_schema: None,
        }],
        schemas: vec![],
    };

    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    let handler = Arc::new(HttpHandler::mock(|_method, _url, _query, _body| {
        panic!("HTTP request should not be made for invalid uuid");
    }));
    let creds = Arc::new(AuthCredentialsMap::new());
    let counter = Arc::new(AtomicUsize::new(0));

    register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

    let result = sb.eval::<Value>(r#"sdk.get_item("not-a-uuid")"#);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("uuid"), "error was: {err}");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test test_enum_validation_rejects_invalid test_format_validation_rejects_invalid_uuid -- --nocapture`
Expected: Both FAIL — mock handler panics because validation isn't wired in yet.

**Step 3: Wire validation into the registry loop**

Add import at top of `registry.rs`:

```rust
use crate::runtime::validate;
```

In the parameter loop, after the nil-skip at line 108 and before string coercion at line 110, add:

```rust
if matches!(value, Value::Nil) {
    continue;
}

// Validate enum and format constraints
let raw_str = lua_value_to_string(&value);
validate::validate_param_value(&func_def.name, param, &raw_str)?;

let str_value = match (&param.param_type, &value) {
```

Note: We compute `raw_str` for validation, then `str_value` still handles the integer coercion. These produce the same result for strings. For integers, `raw_str` via `lua_value_to_string` gives the same string. We could simplify this by validating `str_value` after coercion instead — move the validation call to after line 116:

```rust
let str_value = match (&param.param_type, &value) {
    #[allow(clippy::cast_possible_truncation)]
    (ParamType::Integer, Value::Number(n)) => {
        format!("{}", n.round() as i64)
    }
    _ => lua_value_to_string(&value),
};

// Validate enum and format constraints
validate::validate_param_value(&func_def.name, param, &str_value)?;

match param.location {
```

This is cleaner — validate the coerced string value.

**Step 4: Run tests**

Run: `cargo test`
Expected: All tests pass including the two new registry tests.

**Step 5: Commit**

```
feat: wire format and enum validation into registry
```

---

### Task 8: Update existing test fixtures with `format: None`

This task may already be done as part of Task 1. Verify that all existing tests still pass and no test fixture was missed.

**Step 1: Run the full test suite**

Run: `cargo test`
Expected: All tests pass.

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings.

**Step 3: Commit (only if there are changes)**

```
chore: fix any remaining test fixtures for format field
```

---

### Task 9: Update OPENAPI_GAPS.md

**Files:**
- Modify: `OPENAPI_GAPS.md`

**Step 1: Find and update the enum validation gap entry**

Search for the enum validation or parameter validation entry in `OPENAPI_GAPS.md`. Mark it as completed with a note about format + enum validation.

**Step 2: Commit**

```
docs: mark parameter format/enum validation as completed
```
