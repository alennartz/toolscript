# Frozen Parameters + Table-Based Args Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add frozen parameters (server-side fixed values hidden from the LLM) and switch the Luau SDK calling convention from positional args to named table-based args.

**Architecture:** Frozen params are configured in `code-mcp.toml` at global and per-API levels. During codegen, matching params get a `frozen_value` set on `ParamDef`. Annotations and registry skip/inject them accordingly. The calling convention changes from `sdk.func(arg1, arg2)` to `sdk.func({ key = val })` with body as an optional second argument.

**Tech Stack:** Rust, serde, mlua (Luau), openapiv3

---

### Task 1: Add `frozen_value` field to `ParamDef`

**Files:**
- Modify: `src/codegen/manifest.rs:58-70`

**Step 1: Write the failing test**

In `src/codegen/manifest.rs`, add to the `tests` module:

```rust
#[test]
fn test_param_def_frozen_value_roundtrip() {
    let param = ParamDef {
        name: "api_version".to_string(),
        location: ParamLocation::Query,
        param_type: ParamType::String,
        required: true,
        description: None,
        default: None,
        enum_values: None,
        format: None,
        frozen_value: Some("v2".to_string()),
    };
    let json = serde_json::to_string(&param).unwrap();
    assert!(json.contains("frozen_value"), "frozen_value should be serialized: {json}");
    let roundtripped: ParamDef = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtripped.frozen_value, Some("v2".to_string()));
}

#[test]
fn test_param_def_frozen_value_none_skipped() {
    let param = ParamDef {
        name: "limit".to_string(),
        location: ParamLocation::Query,
        param_type: ParamType::Integer,
        required: false,
        description: None,
        default: None,
        enum_values: None,
        format: None,
        frozen_value: None,
    };
    let json = serde_json::to_string(&param).unwrap();
    assert!(!json.contains("frozen_value"), "None frozen_value should be skipped: {json}");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib codegen::manifest::tests::test_param_def_frozen_value -- -v`
Expected: FAIL — `frozen_value` field does not exist on `ParamDef`

**Step 3: Write minimal implementation**

In `src/codegen/manifest.rs`, add to the `ParamDef` struct after the `format` field:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub frozen_value: Option<String>,
```

Then add `frozen_value: None` to every `ParamDef` construction site across the codebase. Search all files for `ParamDef {` and add the field. Key locations:
- `src/codegen/parser.rs:243` (in `extract_parameters`)
- `src/codegen/manifest.rs` (all test `ParamDef` literals)
- `src/codegen/annotations.rs` (all test `ParamDef` literals)
- `src/runtime/registry.rs` (all test `ParamDef` literals)
- `src/runtime/validate.rs` (the `make_param` helper)
- `src/server/mod.rs` (test `ParamDef` literals)

**Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add src/codegen/manifest.rs src/codegen/parser.rs src/codegen/annotations.rs src/runtime/registry.rs src/runtime/validate.rs src/server/mod.rs
git commit -m "feat: add frozen_value field to ParamDef"
```

---

### Task 2: Add `frozen_params` to config structs

**Files:**
- Modify: `src/config.rs:31-43`

**Step 1: Write the failing test**

In `src/config.rs`, add to the `tests` module:

```rust
#[test]
fn test_load_config_with_frozen_params() {
    let toml_content = r#"
[frozen_params]
api_version = "v2"

[apis.petstore]
spec = "petstore.yaml"

[apis.petstore.frozen_params]
tenant_id = "abc-123"
"#;
    let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
    tmpfile.write_all(toml_content.as_bytes()).unwrap();

    let config = load_config(tmpfile.path()).unwrap();
    let global = config.frozen_params.as_ref().unwrap();
    assert_eq!(global.get("api_version").unwrap(), "v2");

    let api_frozen = config.apis["petstore"].frozen_params.as_ref().unwrap();
    assert_eq!(api_frozen.get("tenant_id").unwrap(), "abc-123");
}

#[test]
fn test_load_config_without_frozen_params() {
    let toml_content = r#"
[apis.petstore]
spec = "petstore.yaml"
"#;
    let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
    tmpfile.write_all(toml_content.as_bytes()).unwrap();

    let config = load_config(tmpfile.path()).unwrap();
    assert!(config.frozen_params.is_none());
    assert!(config.apis["petstore"].frozen_params.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib config::tests::test_load_config_with_frozen_params -- -v`
Expected: FAIL — `frozen_params` field does not exist

**Step 3: Write minimal implementation**

In `src/config.rs`, modify the structs:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigApiEntry {
    pub spec: String,
    #[serde(default)]
    pub auth: Option<ConfigAuth>,
    #[serde(default)]
    pub auth_env: Option<String>,
    #[serde(default)]
    pub frozen_params: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CodeMcpConfig {
    pub apis: HashMap<String, ConfigApiEntry>,
    #[serde(default)]
    pub frozen_params: Option<HashMap<String, String>>,
}
```

Also add `frozen_params: None` to any `ConfigApiEntry` literals in tests (e.g., `test_resolve_config_auth_direct`, `test_resolve_config_auth_basic`, `test_resolve_config_auth_env_ref`).

Add a public helper to merge frozen params:

```rust
/// Merge global and per-API frozen params. Per-API values override global.
pub fn merge_frozen_params(
    global: Option<&HashMap<String, String>>,
    per_api: Option<&HashMap<String, String>>,
) -> HashMap<String, String> {
    let mut merged = global.cloned().unwrap_or_default();
    if let Some(api_params) = per_api {
        merged.extend(api_params.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    merged
}
```

And a test for it:

```rust
#[test]
fn test_merge_frozen_params_precedence() {
    let mut global = HashMap::new();
    global.insert("api_version".to_string(), "v1".to_string());
    global.insert("tenant".to_string(), "default".to_string());

    let mut per_api = HashMap::new();
    per_api.insert("api_version".to_string(), "v2".to_string());

    let merged = merge_frozen_params(Some(&global), Some(&per_api));
    assert_eq!(merged.get("api_version").unwrap(), "v2"); // per-API wins
    assert_eq!(merged.get("tenant").unwrap(), "default"); // global preserved
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib config`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: add frozen_params to config structs"
```

---

### Task 3: Update annotations to table-based signatures + skip frozen

**Files:**
- Modify: `src/codegen/annotations.rs:17-103`

**Step 1: Write the failing tests**

In `src/codegen/annotations.rs`, add/replace tests:

```rust
#[test]
fn test_render_function_table_params() {
    let func = FunctionDef {
        name: "list_pets".to_string(),
        api: "petstore".to_string(),
        tag: None,
        method: HttpMethod::Get,
        path: "/pets".to_string(),
        summary: Some("List all pets".to_string()),
        description: None,
        deprecated: false,
        parameters: vec![
            ParamDef {
                name: "status".to_string(),
                location: ParamLocation::Query,
                param_type: ParamType::String,
                required: false,
                description: Some("Filter by status".to_string()),
                default: None,
                enum_values: None,
                format: None,
                frozen_value: None,
            },
            ParamDef {
                name: "limit".to_string(),
                location: ParamLocation::Query,
                param_type: ParamType::Integer,
                required: true,
                description: Some("Max items".to_string()),
                default: None,
                enum_values: None,
                format: None,
                frozen_value: None,
            },
        ],
        request_body: None,
        response_schema: Some("Pet".to_string()),
    };

    let output = render_function_annotation(&func);
    assert!(
        output.contains("function sdk.list_pets(params: { status: string?, limit: number }): Pet end"),
        "Should use table-based params. Got:\n{output}"
    );
}

#[test]
fn test_render_function_table_params_with_body() {
    let func = FunctionDef {
        name: "create_pet".to_string(),
        api: "petstore".to_string(),
        tag: None,
        method: HttpMethod::Post,
        path: "/pets".to_string(),
        summary: None,
        description: None,
        deprecated: false,
        parameters: vec![ParamDef {
            name: "tag".to_string(),
            location: ParamLocation::Query,
            param_type: ParamType::String,
            required: false,
            description: None,
            default: None,
            enum_values: None,
            format: None,
            frozen_value: None,
        }],
        request_body: Some(RequestBodyDef {
            content_type: "application/json".to_string(),
            schema: "NewPet".to_string(),
            required: true,
            description: None,
        }),
        response_schema: Some("Pet".to_string()),
    };

    let output = render_function_annotation(&func);
    assert!(
        output.contains("function sdk.create_pet(params: { tag: string? }, body: NewPet): Pet end"),
        "Should have params table + body. Got:\n{output}"
    );
}

#[test]
fn test_render_function_all_frozen_no_body() {
    let func = FunctionDef {
        name: "get_status".to_string(),
        api: "myapi".to_string(),
        tag: None,
        method: HttpMethod::Get,
        path: "/status".to_string(),
        summary: None,
        description: None,
        deprecated: false,
        parameters: vec![ParamDef {
            name: "api_version".to_string(),
            location: ParamLocation::Query,
            param_type: ParamType::String,
            required: true,
            description: None,
            default: None,
            enum_values: None,
            format: None,
            frozen_value: Some("v2".to_string()),
        }],
        request_body: None,
        response_schema: None,
    };

    let output = render_function_annotation(&func);
    assert!(
        output.contains("function sdk.get_status() end"),
        "All-frozen with no body should have no args. Got:\n{output}"
    );
    assert!(
        !output.contains("api_version"),
        "Frozen param should not appear. Got:\n{output}"
    );
}

#[test]
fn test_render_function_all_frozen_with_body() {
    let func = FunctionDef {
        name: "create_thing".to_string(),
        api: "myapi".to_string(),
        tag: None,
        method: HttpMethod::Post,
        path: "/things".to_string(),
        summary: None,
        description: None,
        deprecated: false,
        parameters: vec![ParamDef {
            name: "api_version".to_string(),
            location: ParamLocation::Query,
            param_type: ParamType::String,
            required: true,
            description: None,
            default: None,
            enum_values: None,
            format: None,
            frozen_value: Some("v2".to_string()),
        }],
        request_body: Some(RequestBodyDef {
            content_type: "application/json".to_string(),
            schema: "NewThing".to_string(),
            required: true,
            description: None,
        }),
        response_schema: None,
    };

    let output = render_function_annotation(&func);
    assert!(
        output.contains("function sdk.create_thing(body: NewThing) end"),
        "All-frozen with body should have body as sole arg. Got:\n{output}"
    );
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib codegen::annotations::tests::test_render_function_table_params -- -v`
Expected: FAIL — signatures still positional

**Step 3: Write implementation**

Rewrite `render_function_annotation` in `src/codegen/annotations.rs`:

1. Filter params to only non-frozen: `let visible_params: Vec<_> = func.parameters.iter().filter(|p| p.frozen_value.is_none()).collect();`
2. Build `@param` doc lines only for visible params.
3. Build the signature based on the four cases:
   - `has_visible_params && has_body`: `(params: { ... }, body: Type)`
   - `has_visible_params && !has_body`: `(params: { ... })`
   - `!has_visible_params && has_body`: `(body: Type)`
   - `!has_visible_params && !has_body`: `()`
4. For the params table type, render each visible param as `name: type` (with `?` suffix for optional).

The params table type format: `{ status: string?, limit: number }`

**Step 4: Update all existing annotation tests**

Update every existing test that asserts on function signature format to match the new table-based output. Key tests to update:
- `test_render_function_annotation` — signature changes to table-based
- `test_render_function_with_optional_params` — params go in table
- `test_render_function_with_enum_param` — enum in table
- `test_render_function_deprecated` — no params, stays `()`
- `test_render_function_with_request_body` — body-only case
- `test_generate_annotation_files` — check content matches new format

**Step 5: Run all tests**

Run: `cargo test --lib codegen::annotations`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add src/codegen/annotations.rs
git commit -m "feat: switch annotations to table-based signatures, skip frozen params"
```

---

### Task 4: Thread frozen config through the generate pipeline

**Files:**
- Modify: `src/codegen/generate.rs:11-51`

**Step 1: Write the failing test**

In `src/codegen/generate.rs`, add to the `tests` module:

```rust
#[tokio::test]
async fn test_generate_with_frozen_params() {
    use std::collections::HashMap;
    let output_dir = tempfile::tempdir().unwrap();
    let mut frozen = HashMap::new();
    frozen.insert("limit".to_string(), "10".to_string());

    generate(
        &[SpecInput {
            name: Some("petstore".to_string()),
            source: "testdata/petstore.yaml".to_string(),
        }],
        output_dir.path(),
        &HashMap::new(),
        &{
            let mut m = HashMap::new();
            m.insert("petstore".to_string(), frozen);
            m
        },
    )
    .await
    .unwrap();

    let manifest: Manifest = serde_json::from_str(
        &std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap(),
    )
    .unwrap();

    // Find the list_pets function and check that limit has frozen_value
    let list_pets = manifest.functions.iter().find(|f| f.name == "list_pets").unwrap();
    let limit_param = list_pets.parameters.iter().find(|p| p.name == "limit").unwrap();
    assert_eq!(limit_param.frozen_value, Some("10".to_string()));

    // Other params should not be frozen
    for param in &list_pets.parameters {
        if param.name != "limit" {
            assert_eq!(param.frozen_value, None, "param {} should not be frozen", param.name);
        }
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test codegen_integration test_generate_with_frozen_params -- -v` (or in generate module)
Expected: FAIL — `generate` doesn't accept frozen params

**Step 3: Write implementation**

Change the `generate` function signature to:

```rust
pub async fn generate(
    specs: &[SpecInput],
    output_dir: &Path,
    global_frozen: &HashMap<String, String>,
    per_api_frozen: &HashMap<String, HashMap<String, String>>,
) -> Result<()>
```

After `spec_to_manifest` returns, apply frozen values:

```rust
let api_frozen = crate::config::merge_frozen_params(
    if global_frozen.is_empty() { None } else { Some(global_frozen) },
    per_api_frozen.get(&api_name).map(|m| m.as_ref()),
);
if !api_frozen.is_empty() {
    for func in &mut manifest.functions {
        for param in &mut func.parameters {
            if let Some(value) = api_frozen.get(&param.name) {
                param.frozen_value = Some(value.clone());
            }
        }
    }
}
```

Update all existing callers of `generate()`:
- `src/main.rs:42` — pass empty maps for now (will be wired in Task 6)
- `src/main.rs:95` — pass empty maps for now
- All tests in `generate.rs` and `codegen_integration.rs` — pass empty maps

Import `std::collections::HashMap` where needed.

**Step 4: Run tests**

Run: `cargo test`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add src/codegen/generate.rs src/main.rs tests/codegen_integration.rs
git commit -m "feat: thread frozen config through generate pipeline"
```

---

### Task 5: Update registry to table-based extraction + frozen injection

**Files:**
- Modify: `src/runtime/registry.rs:60-198`

This is the largest task. The Lua function currently extracts positional args. It needs to:
1. Determine the calling convention (4 cases based on visible params and body).
2. Extract param values from a Lua table by name.
3. For frozen params, use the frozen value directly.

**Step 1: Write the failing tests**

Add new tests in `src/runtime/registry.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_table_based_params() {
    let captured_query = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let captured_query_clone = Arc::clone(&captured_query);

    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    let manifest = test_manifest();
    let handler = Arc::new(HttpHandler::mock(move |_method, _url, query, _body| {
        *captured_query_clone.lock().unwrap() = query.to_vec();
        Ok(serde_json::json!([]))
    }));
    let creds = Arc::new(AuthCredentialsMap::new());
    let counter = Arc::new(AtomicUsize::new(0));

    register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

    sb.eval::<Value>(r#"sdk.list_pets({ status = "available", limit = 10 })"#)
        .unwrap();

    let query = captured_query.lock().unwrap().clone();
    assert_eq!(query.len(), 2);
    assert!(query.iter().any(|(k, v)| k == "status" && v == "available"));
    assert!(query.iter().any(|(k, v)| k == "limit" && v == "10"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_table_based_path_param() {
    let captured_url = Arc::new(Mutex::new(String::new()));
    let captured_url_clone = Arc::clone(&captured_url);

    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    let manifest = test_manifest();
    let handler = Arc::new(HttpHandler::mock(move |_method, url, _query, _body| {
        *captured_url_clone.lock().unwrap() = url.to_string();
        Ok(serde_json::json!({"id": "456"}))
    }));
    let creds = Arc::new(AuthCredentialsMap::new());
    let counter = Arc::new(AtomicUsize::new(0));

    register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

    sb.eval::<Value>(r#"sdk.get_pet({ pet_id = "456" })"#).unwrap();

    let url = captured_url.lock().unwrap().clone();
    assert_eq!(url, "https://petstore.example.com/v1/pets/456");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_frozen_param_injected() {
    let captured_query = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let captured_query_clone = Arc::clone(&captured_query);

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
            parameters: vec![
                ParamDef {
                    name: "api_version".to_string(),
                    location: ParamLocation::Query,
                    param_type: ParamType::String,
                    required: true,
                    description: None,
                    default: None,
                    enum_values: None,
                    format: None,
                    frozen_value: Some("v2".to_string()),
                },
                ParamDef {
                    name: "limit".to_string(),
                    location: ParamLocation::Query,
                    param_type: ParamType::Integer,
                    required: false,
                    description: None,
                    default: None,
                    enum_values: None,
                    format: None,
                    frozen_value: None,
                },
            ],
            request_body: None,
            response_schema: None,
        }],
        schemas: vec![],
    };

    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    let handler = Arc::new(HttpHandler::mock(move |_method, _url, query, _body| {
        *captured_query_clone.lock().unwrap() = query.to_vec();
        Ok(serde_json::json!([]))
    }));
    let creds = Arc::new(AuthCredentialsMap::new());
    let counter = Arc::new(AtomicUsize::new(0));

    register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

    // Only pass limit — api_version is frozen
    sb.eval::<Value>(r#"sdk.list_items({ limit = 5 })"#).unwrap();

    let query = captured_query.lock().unwrap().clone();
    assert!(query.iter().any(|(k, v)| k == "api_version" && v == "v2"),
        "Frozen param should be injected. Got: {query:?}");
    assert!(query.iter().any(|(k, v)| k == "limit" && v == "5"),
        "Non-frozen param should come from table. Got: {query:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_all_frozen_no_body_no_args() {
    let captured_query = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let captured_query_clone = Arc::clone(&captured_query);

    let manifest = Manifest {
        apis: vec![ApiConfig {
            name: "testapi".to_string(),
            base_url: "https://api.example.com".to_string(),
            description: None,
            version: None,
            auth: None,
        }],
        functions: vec![FunctionDef {
            name: "get_status".to_string(),
            api: "testapi".to_string(),
            tag: None,
            method: HttpMethod::Get,
            path: "/status".to_string(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: vec![ParamDef {
                name: "api_version".to_string(),
                location: ParamLocation::Query,
                param_type: ParamType::String,
                required: true,
                description: None,
                default: None,
                enum_values: None,
                format: None,
                frozen_value: Some("v2".to_string()),
            }],
            request_body: None,
            response_schema: None,
        }],
        schemas: vec![],
    };

    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    let handler = Arc::new(HttpHandler::mock(move |_method, _url, query, _body| {
        *captured_query_clone.lock().unwrap() = query.to_vec();
        Ok(serde_json::json!({"status": "ok"}))
    }));
    let creds = Arc::new(AuthCredentialsMap::new());
    let counter = Arc::new(AtomicUsize::new(0));

    register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

    // No args at all
    sb.eval::<Value>("sdk.get_status()").unwrap();

    let query = captured_query.lock().unwrap().clone();
    assert!(query.iter().any(|(k, v)| k == "api_version" && v == "v2"),
        "Frozen param should still be injected. Got: {query:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_all_frozen_with_body_as_sole_arg() {
    let captured_body = Arc::new(Mutex::new(None::<serde_json::Value>));
    let captured_body_clone = Arc::clone(&captured_body);
    let captured_query = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let captured_query_clone = Arc::clone(&captured_query);

    let manifest = Manifest {
        apis: vec![ApiConfig {
            name: "testapi".to_string(),
            base_url: "https://api.example.com".to_string(),
            description: None,
            version: None,
            auth: None,
        }],
        functions: vec![FunctionDef {
            name: "create_thing".to_string(),
            api: "testapi".to_string(),
            tag: None,
            method: HttpMethod::Post,
            path: "/things".to_string(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: vec![ParamDef {
                name: "api_version".to_string(),
                location: ParamLocation::Query,
                param_type: ParamType::String,
                required: true,
                description: None,
                default: None,
                enum_values: None,
                format: None,
                frozen_value: Some("v2".to_string()),
            }],
            request_body: Some(RequestBodyDef {
                content_type: "application/json".to_string(),
                schema: "NewThing".to_string(),
                required: true,
                description: None,
            }),
            response_schema: None,
        }],
        schemas: vec![],
    };

    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    let handler = Arc::new(HttpHandler::mock(move |_method, _url, query, body| {
        *captured_query_clone.lock().unwrap() = query.to_vec();
        *captured_body_clone.lock().unwrap() = body.cloned();
        Ok(serde_json::json!({"id": "1"}))
    }));
    let creds = Arc::new(AuthCredentialsMap::new());
    let counter = Arc::new(AtomicUsize::new(0));

    register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

    // Body is the sole arg (no params table since all frozen)
    sb.eval::<Value>(r#"sdk.create_thing({ name = "Widget" })"#).unwrap();

    let query = captured_query.lock().unwrap().clone();
    assert!(query.iter().any(|(k, v)| k == "api_version" && v == "v2"));

    let body = captured_body.lock().unwrap().clone();
    assert!(body.is_some());
    assert_eq!(body.unwrap()["name"], "Widget");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib runtime::registry::tests::test_table_based_params -- -v`
Expected: FAIL — still using positional extraction

**Step 3: Write implementation**

Rewrite the Lua function closure in `register_functions` (lines 60-198):

```rust
let lua_fn = lua.create_function(move |lua, args: MultiValue| {
    let func_def = &func_def_clone;
    let handler = &handler_clone;
    let credentials = &credentials_clone;
    let counter = &counter_clone;

    // Check API call limit
    let current_count = counter.load(Ordering::SeqCst);
    if let Some(max) = max_calls
        && current_count >= max
    {
        return Err(mlua::Error::external(anyhow::anyhow!(
            "API call limit exceeded (max {max} calls)",
        )));
    }

    let arg_values: Vec<Value> = args.into_iter().collect();

    // Determine calling convention
    let has_visible_params = func_def.parameters.iter().any(|p| p.frozen_value.is_none());
    let has_body = func_def.request_body.is_some();

    // Extract params table and body based on calling convention
    let params_table: Option<mlua::Table> = if has_visible_params {
        let table_val = arg_values.first().cloned().unwrap_or(Value::Nil);
        match table_val {
            Value::Table(t) => Some(t),
            Value::Nil => None,
            _ => return Err(mlua::Error::external(anyhow::anyhow!(
                "expected table as first argument to '{}', got {}",
                func_def.name,
                table_val.type_name()
            ))),
        }
    } else {
        None
    };

    let body_arg_idx = if has_visible_params { 1 } else { 0 };

    // Build path, query, and header params
    let mut url = base_url.clone();
    let mut path = func_def.path.clone();
    let mut query_params: Vec<(String, String)> = Vec::new();
    let mut header_params: Vec<(String, String)> = Vec::new();

    for param in &func_def.parameters {
        let str_value = if let Some(ref frozen) = param.frozen_value {
            // Frozen param — use configured value directly, skip validation
            frozen.clone()
        } else {
            // Non-frozen — extract from table
            let value: Value = params_table
                .as_ref()
                .map(|t| t.get::<Value>(param.name.as_str()))
                .transpose()?
                .unwrap_or(Value::Nil);

            if param.required && matches!(value, Value::Nil) {
                return Err(mlua::Error::external(anyhow::anyhow!(
                    "missing required parameter '{}' for function '{}'",
                    param.name,
                    func_def.name
                )));
            }

            if matches!(value, Value::Nil) {
                continue;
            }

            let str_val = match (&param.param_type, &value) {
                #[allow(clippy::cast_possible_truncation)]
                (ParamType::Integer, Value::Number(n)) => {
                    format!("{}", n.round() as i64)
                }
                _ => lua_value_to_string(&value),
            };

            // Validate enum and format constraints
            validate::validate_param_value(&func_def.name, param, &str_val)?;

            str_val
        };

        match param.location {
            ParamLocation::Path => {
                path = path.replace(&format!("{{{}}}", param.name), &str_value);
            }
            ParamLocation::Query => {
                query_params.push((param.name.clone(), str_value));
            }
            ParamLocation::Header => {
                header_params.push((param.name.clone(), str_value));
            }
        }
    }

    url.push_str(&path);

    // Extract request body
    let body: Option<serde_json::Value> = if has_body {
        if body_arg_idx < arg_values.len() {
            let body_val = arg_values[body_arg_idx].clone();
            if matches!(body_val, Value::Nil) {
                None
            } else {
                let json_body: serde_json::Value = lua.from_value(body_val).map_err(|e| {
                    mlua::Error::external(anyhow::anyhow!(
                        "failed to serialize request body: {e}",
                    ))
                })?;
                Some(json_body)
            }
        } else {
            None
        }
    } else {
        None
    };

    // ... rest is unchanged (method, creds, counter, HTTP call, response conversion)
```

**Step 4: Update existing registry tests**

All existing tests that call `sdk.func(positional, args)` need to be updated to `sdk.func({ key = val })` or `sdk.func({ key = val }, body)` syntax.

Key tests to update:
- `test_register_and_call_function`: `sdk.get_pet("123")` → `sdk.get_pet({ pet_id = "123" })`
- `test_path_param_substitution`: same change
- `test_query_params_passed`: `sdk.list_pets("available", 10)` → `sdk.list_pets({ status = "available", limit = 10 })`
- `test_missing_required_param_errors`: `sdk.get_pet()` → `sdk.get_pet({})` or `sdk.get_pet()`
- `test_optional_param_can_be_nil`: `sdk.list_pets()` → stays the same (no visible required params)
- `test_request_body_sent`: `sdk.create_pet({name = "Buddy"})` → stays the same (no params, body is sole arg)
- `test_optional_header_param_omitted`: `sdk.get_thing("abc-123")` → `sdk.get_thing({ id = "abc-123" })`
- `test_header_param_integer_serialization`: `sdk.do_thing(50)` → `sdk.do_thing({ ["X-Page-Size"] = 50 })`
- `test_header_params_sent`: `sdk.do_thing("trace-123", 10)` → `sdk.do_thing({ ["X-Request-ID"] = "trace-123", limit = 10 })`
- `test_enum_validation_rejects_invalid`: `sdk.list_items("deleted")` → `sdk.list_items({ status = "deleted" })`
- `test_format_validation_rejects_invalid_uuid`: `sdk.get_item("not-a-uuid")` → `sdk.get_item({ id = "not-a-uuid" })`

**Step 5: Run all tests**

Run: `cargo test`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add src/runtime/registry.rs
git commit -m "feat: switch registry to table-based args, add frozen param injection"
```

---

### Task 6: Wire frozen config through main.rs

**Files:**
- Modify: `src/main.rs:33-126`

**Step 1: Write the failing test**

This is a wiring task — the unit tests are already covered. Verify by doing a manual run or checking compilation.

**Step 2: Write implementation**

In `main.rs`, update both `Command::Generate` and `Command::Run` to extract and pass frozen params:

For `Command::Generate`:
```rust
Command::Generate { specs, output, config } => {
    let (spec_inputs, config_obj) = if let Some(path) = config.as_deref() {
        let config = load_config(path)?;
        let inputs = config.apis.iter().map(|(name, entry)| SpecInput {
            name: Some(name.clone()),
            source: entry.spec.clone(),
        }).collect();
        (inputs, Some(config))
    } else {
        (specs.iter().map(|s| parse_spec_arg(s)).collect(), None)
    };

    let (global_frozen, per_api_frozen) = extract_frozen_params(config_obj.as_ref());
    generate(&spec_inputs, &output, &global_frozen, &per_api_frozen).await?;
    eprintln!("Generated output to {}", output.display());
    Ok(())
}
```

For `Command::Run`:
```rust
let (global_frozen, per_api_frozen) = extract_frozen_params(config_obj.as_ref());
generate(&spec_inputs, tmpdir.path(), &global_frozen, &per_api_frozen).await?;
```

Add a helper function:
```rust
fn extract_frozen_params(
    config: Option<&CodeMcpConfig>,
) -> (HashMap<String, String>, HashMap<String, HashMap<String, String>>) {
    let Some(config) = config else {
        return (HashMap::new(), HashMap::new());
    };
    let global = config.frozen_params.clone().unwrap_or_default();
    let per_api: HashMap<String, HashMap<String, String>> = config
        .apis
        .iter()
        .filter_map(|(name, entry)| {
            entry.frozen_params.as_ref().map(|fp| (name.clone(), fp.clone()))
        })
        .collect();
    (global, per_api)
}
```

Note: `resolve_spec_inputs` for `Generate` needs updating to also return the config object (currently it doesn't). Refactor to match the `resolve_run_inputs` pattern or inline the logic.

**Step 3: Run tests**

Run: `cargo test`
Expected: ALL PASS

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire frozen params from config through to codegen"
```

---

### Task 7: Update server annotation cache for frozen params

**Files:**
- Modify: `src/server/mod.rs:44-55`

The server pre-renders annotations at startup using `render_function_annotation`. Since Task 3 already updated that function to skip frozen params, the annotation cache will automatically be correct. However, the `search_docs` function in `tools.rs` iterates `func.parameters` for keyword matching — frozen params should be excluded from search results too.

**Step 1: Write the failing test**

In `src/server/mod.rs` tests:

```rust
#[test]
fn test_frozen_params_hidden_from_docs() {
    let mut manifest = test_manifest();
    // Freeze the "limit" param on list_pets
    for func in &mut manifest.functions {
        if func.name == "list_pets" {
            for param in &mut func.parameters {
                if param.name == "limit" {
                    param.frozen_value = Some("20".to_string());
                }
            }
        }
    }
    let server = CodeMcpServer::new(
        manifest,
        Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
        AuthCredentialsMap::new(),
        ExecutorConfig::default(),
    );

    let docs = tools::get_function_docs_impl(&server, "list_pets").unwrap();
    assert!(!docs.contains("limit"), "Frozen param 'limit' should not appear in docs. Got:\n{docs}");
}
```

**Step 2: Run test to verify it fails/passes**

Run: `cargo test --lib server::tests::test_frozen_params_hidden_from_docs`
Expected: PASS (if annotations already skip frozen) — if so, this is a validation test. If FAIL, need additional filtering.

**Step 3: Also update `search_docs_impl` in `tools.rs`**

In `src/server/tools.rs`, the param search loop (line 127-131) should skip frozen params:

```rust
for param in &func.parameters {
    if param.frozen_value.is_some() {
        continue; // Skip frozen params from search
    }
    if param.name.to_lowercase().contains(&query_lower) {
        // ...
    }
}
```

**Step 4: Run tests**

Run: `cargo test`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add src/server/mod.rs src/server/tools.rs
git commit -m "feat: hide frozen params from search results"
```

---

### Task 8: Integration test

**Files:**
- Modify: `tests/codegen_integration.rs`

**Step 1: Write the integration test**

```rust
#[tokio::test]
async fn test_frozen_params_end_to_end() {
    use std::collections::HashMap;

    let output_dir = tempfile::tempdir().unwrap();
    let mut per_api_frozen = HashMap::new();
    let mut petstore_frozen = HashMap::new();
    petstore_frozen.insert("limit".to_string(), "25".to_string());
    per_api_frozen.insert("petstore".to_string(), petstore_frozen);

    code_mcp::codegen::generate::generate(
        &[SpecInput {
            name: Some("petstore".to_string()),
            source: "testdata/petstore.yaml".to_string(),
        }],
        output_dir.path(),
        &HashMap::new(),
        &per_api_frozen,
    )
    .await
    .unwrap();

    // Check manifest has frozen_value set
    let manifest: code_mcp::codegen::manifest::Manifest = serde_json::from_str(
        &std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap(),
    )
    .unwrap();

    let list_pets = manifest.functions.iter().find(|f| f.name == "list_pets").unwrap();
    let limit = list_pets.parameters.iter().find(|p| p.name == "limit").unwrap();
    assert_eq!(limit.frozen_value, Some("25".to_string()));

    // Check that Luau annotations don't mention the frozen param
    let sdk_dir = output_dir.path().join("sdk");
    for entry in std::fs::read_dir(&sdk_dir).unwrap() {
        let entry = entry.unwrap();
        let content = std::fs::read_to_string(entry.path()).unwrap();
        if content.contains("function sdk.list_pets") {
            assert!(
                !content.contains("limit"),
                "Frozen param 'limit' should not appear in Luau annotations. Got:\n{content}"
            );
        }
    }
}
```

**Step 2: Run test**

Run: `cargo test --test codegen_integration test_frozen_params_end_to_end`
Expected: PASS

**Step 3: Commit**

```bash
git add tests/codegen_integration.rs
git commit -m "test: add frozen params end-to-end integration test"
```

---

### Task 9: Update README

**Files:**
- Modify: `README.md`

**Step 1: Add frozen params section**

After the "Authentication" section, add a "Frozen Parameters" section documenting:
- What frozen params are
- Config syntax (global + per-API)
- Precedence rules
- Example use cases (API version pinning, tenant ID)

**Step 2: Update Luau script examples**

The example script in the README (around line 67) already uses table syntax:
```lua
local pets = sdk.list_pets({ limit = 5 })
```

Verify all examples use table syntax. If any use positional, update them.

**Step 3: Commit**

```bash
git add README.md
git commit -m "docs: add frozen params docs and update SDK examples"
```
