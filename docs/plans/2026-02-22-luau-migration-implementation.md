# Luau Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrate the scripting runtime from Lua 5.4 to Luau for native sandboxing, interrupt-based timeouts, and native type annotations.

**Architecture:** Switch the mlua crate feature from `lua54` to `luau`. Rewrite sandbox to use `lua.sandbox(true)` instead of manual deny-listing. Replace instruction hooks with `set_interrupt()`. Rewrite annotation output from EmmyLua comments to Luau native type syntax.

**Tech Stack:** Rust, mlua 0.10 (luau feature), Luau VM, serde_json

**Design doc:** `docs/plans/2026-02-22-luau-migration-design.md`

---

### Task 1: Switch mlua feature flag and fix sandbox compilation

The Cargo.toml change breaks compilation until sandbox.rs and executor.rs are updated. These must change together.

**Files:**
- Modify: `Cargo.toml:13`
- Modify: `src/runtime/sandbox.rs` (full rewrite of `new()` and `format_lua_value`)

**Step 1: Update Cargo.toml**

Change line 13 from:
```toml
mlua = { version = "0.10", features = ["lua54", "vendored", "async", "send", "serialize"] }
```
To:
```toml
mlua = { version = "0.10", features = ["luau", "async", "send", "serialize"] }
```

**Step 2: Rewrite sandbox.rs**

Replace the full `Sandbox::new` method. The new version uses `Lua::new()` + `lua.sandbox(true)`.

Update the import line from:
```rust
use mlua::{FromLua, Lua, LuaOptions, MultiValue, StdLib, Value};
```
To:
```rust
use mlua::{FromLua, Lua, MultiValue, Value};
```

Replace the doc comment on `Sandbox`:
```rust
/// A locked-down Luau environment using native sandbox mode.
```

Replace the doc comment on `Sandbox::new`:
```rust
/// Create a new sandboxed Luau environment.
///
/// Uses Luau's native sandbox mode which makes all globals and metatables
/// read-only, creates isolated per-script environments, and restricts
/// `collectgarbage`. Custom `print()`, `json`, and `sdk` globals are
/// injected after sandboxing.
```

Replace the entire `Sandbox::new` body with:
```rust
pub fn new(config: SandboxConfig) -> anyhow::Result<Self> {
    let lua = Lua::new();

    // Set memory limit before sandboxing
    if let Some(limit) = config.memory_limit {
        lua.set_memory_limit(limit)?;
    }

    // Shared log buffer for captured print output
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // Override print() to capture output
    let logs_clone = Arc::clone(&logs);
    let print_fn = lua.create_function(move |_, args: MultiValue| {
        let parts: Vec<String> = args.iter().map(format_lua_value).collect();
        let line = parts.join("\t");
        if let Ok(mut logs) = logs_clone.lock() {
            logs.push(line);
        }
        Ok(())
    })?;
    lua.globals().set("print", print_fn)?;

    // Add json.encode() and json.decode() — Rust-backed via serde
    let json_table = lua.create_table()?;

    let encode_fn = lua.create_function(|lua, value: Value| {
        use mlua::LuaSerdeExt;
        let json_value: serde_json::Value = lua.from_value(value)?;
        serde_json::to_string(&json_value).map_err(mlua::Error::external)
    })?;
    json_table.set("encode", encode_fn)?;

    let decode_fn = lua.create_function(|lua, s: String| {
        use mlua::LuaSerdeExt;
        let json_value: serde_json::Value =
            serde_json::from_str(&s).map_err(mlua::Error::external)?;
        lua.to_value(&json_value)
    })?;
    json_table.set("decode", decode_fn)?;

    lua.globals().set("json", json_table)?;

    // Create empty sdk table (will be populated by registry)
    let sdk_table = lua.create_table()?;
    lua.globals().set("sdk", sdk_table)?;

    // Enable Luau sandbox mode AFTER setting up custom globals.
    // This makes all globals (including our custom ones) read-only and
    // activates per-script isolated environments.
    lua.sandbox(true)?;

    Ok(Self { lua, logs })
}
```

**Key detail:** `lua.sandbox(true)` must be called AFTER setting up print/json/sdk globals, because sandbox mode makes globals read-only. If called before, we can't inject our globals.

**Step 3: Update format_lua_value**

Remove the `Value::Integer` branch since Luau only produces `Value::Number`. Update the `Value::Number` branch to handle whole numbers:

```rust
fn format_lua_value(value: &Value) -> String {
    match value {
        Value::Nil => "nil".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Number(n) => {
            #[allow(
                clippy::float_cmp,
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation
            )]
            if *n == (*n as i64) as f64 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        Value::String(s) => s.to_string_lossy(),
        Value::Table(_) => "table".to_string(),
        Value::Function(_) => "function".to_string(),
        Value::UserData(_) | Value::LightUserData(_) => "userdata".to_string(),
        Value::Thread(_) => "thread".to_string(),
        Value::Error(e) => format!("error: {e}"),
        _ => "unknown".to_string(),
    }
}
```

Note: `Value::Other` is renamed to a wildcard `_` catch-all since Luau may have different variant names. The `Value::Integer` branch had `n.to_string()` which formats as `"42"`. The new `Value::Number` branch for whole numbers uses `format!("{}", *n as i64)` to produce the same `"42"` output instead of `"42.0"`.

**Step 4: Update sandbox tests**

The `test_sandbox_allows_math_lib` test asserts `i64` return type, but Luau returns all numbers as floats. Change:
```rust
#[test]
fn test_sandbox_allows_math_lib() {
    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    let result: f64 = sb.eval("return math.floor(3.7)").unwrap();
    assert!((result - 3.0).abs() < f64::EPSILON);
}
```

The `test_sandbox_blocks_os_execute` test needs updating because Luau doesn't have a full `os` table — it has a limited `os` with only `clock`/`difftime`/`time`. The script `os.execute('ls')` will error because `execute` doesn't exist on Luau's limited `os`. The test still passes (it asserts error), but update the test name/comment for clarity:
```rust
#[test]
fn test_sandbox_blocks_os_execute() {
    let sb = Sandbox::new(SandboxConfig::default()).unwrap();
    // Luau's os table only has clock/difftime/time — execute doesn't exist
    let result = sb.eval::<Value>("return os.execute('ls')");
    assert!(result.is_err());
}
```

The `test_sandbox_blocks_io` test: Luau doesn't have `io` at all. The error will be different (attempting to index nil) but it still errors. Keep test as-is.

The `test_sandbox_blocks_loadfile`, `test_sandbox_blocks_dofile`, `test_sandbox_blocks_require`, `test_sandbox_blocks_string_dump` tests all still error in Luau because these globals don't exist. Keep tests as-is.

**Step 5: Verify sandbox compiles**

Run: `cargo check 2>&1 | head -30`
Expected: Errors only from `executor.rs` (still uses `HookTriggers`), not from `sandbox.rs`.

---

### Task 2: Update executor for Luau interrupt

**Files:**
- Modify: `src/runtime/executor.rs:1-170` (imports, execute method, lua_value_to_json)

**Step 1: Fix imports**

Change line 5 from:
```rust
use mlua::{HookTriggers, LuaSerdeExt, Value, VmState};
```
To:
```rust
use mlua::{LuaSerdeExt, Value, VmState};
```

**Step 2: Replace set_hook with set_interrupt**

In the `execute` method, replace lines 107-121:
```rust
// 4. Set up timeout via instruction hook
let effective_timeout = timeout_ms.unwrap_or(self.config.timeout_ms);
let deadline = Instant::now() + std::time::Duration::from_millis(effective_timeout);
sandbox.lua().set_hook(
    HookTriggers::new().every_nth_instruction(1000),
    move |_lua, _debug| {
        if Instant::now() >= deadline {
            Err(mlua::Error::external(anyhow::anyhow!(
                "script execution timed out"
            )))
        } else {
            Ok(VmState::Continue)
        }
    },
);
```

With:
```rust
// 4. Set up timeout via Luau interrupt
let effective_timeout = timeout_ms.unwrap_or(self.config.timeout_ms);
let deadline = Instant::now() + std::time::Duration::from_millis(effective_timeout);
sandbox.lua().set_interrupt(move |_lua| {
    if Instant::now() >= deadline {
        Err(mlua::Error::external(anyhow::anyhow!(
            "script execution timed out"
        )))
    } else {
        Ok(VmState::Continue)
    }
});
```

**Step 3: Remove hook cleanup**

Delete line 128-129:
```rust
// 6. Remove the hook
sandbox.lua().remove_hook();
```

Update the comment numbering on subsequent steps (7→6, 8→7).

**Step 4: Update lua_value_to_json**

Replace the function (lines 157-170) with:
```rust
/// Convert a Lua `Value` to `serde_json::Value`.
fn lua_value_to_json(lua: &mlua::Lua, value: Value) -> anyhow::Result<serde_json::Value> {
    match value {
        Value::Boolean(b) => Ok(serde_json::Value::Bool(b)),
        #[allow(clippy::cast_possible_truncation)]
        Value::Number(n) => {
            if n.fract() == 0.0 && (i64::MIN as f64..=i64::MAX as f64).contains(&n) {
                Ok(serde_json::json!(n as i64))
            } else {
                Ok(serde_json::json!(n))
            }
        }
        Value::String(s) => Ok(serde_json::Value::String(s.to_string_lossy())),
        Value::Table(_) => {
            let json: serde_json::Value = lua.from_value(value)?;
            Ok(json)
        }
        _ => Ok(serde_json::Value::Null),
    }
}
```

**Step 5: Verify compilation and run tests**

Run: `cargo test --lib runtime::executor 2>&1`
Expected: All executor tests pass. The `test_execute_returns_result` test asserts `serde_json::json!(42)` — our whole-number detection ensures `42.0` from Luau serializes as `42` in JSON, so this passes unchanged.

**Step 6: Commit**

```bash
git add Cargo.toml src/runtime/sandbox.rs src/runtime/executor.rs
git commit -m "feat: migrate runtime from Lua 5.4 to Luau

Switch mlua feature flag from lua54 to luau. Replace manual
deny-list sandbox with Luau's native sandbox(true). Replace
instruction hooks with set_interrupt() for timeouts. Handle
Luau's unified number type (no distinct integers)."
```

---

### Task 3: Update registry integer handling

**Files:**
- Modify: `src/runtime/registry.rs:6,89-121,194-203`

**Step 1: Add ParamType import**

Change line 6 from:
```rust
use crate::codegen::manifest::{Manifest, ParamLocation};
```
To:
```rust
use crate::codegen::manifest::{Manifest, ParamLocation, ParamType};
```

**Step 2: Add type-informed integer rounding in param loop**

In the `register_functions` closure, replace lines 109-121 (the param value stringification and location matching) with:

```rust
                let str_value = match (&param.param_type, &value) {
                    #[allow(clippy::cast_possible_truncation)]
                    (ParamType::Integer, Value::Number(n)) => {
                        format!("{}", n.round() as i64)
                    }
                    _ => lua_value_to_string(&value),
                };

                match param.location {
                    ParamLocation::Path => {
                        path = path.replace(&format!("{{{}}}", param.name), &str_value);
                    }
                    ParamLocation::Query => {
                        query_params.push((param.name.clone(), str_value));
                    }
                    ParamLocation::Header => {
                        // Headers are handled at HTTP level; for now skip
                    }
                }
```

**Step 3: Update lua_value_to_string**

Replace the function (lines 194-203) with:
```rust
/// Convert a Lua value to a string for URL parameter encoding.
fn lua_value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_string_lossy(),
        #[allow(clippy::cast_possible_truncation)]
        Value::Number(n) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        Value::Boolean(b) => b.to_string(),
        _ => String::new(),
    }
}
```

**Step 4: Run registry tests**

Run: `cargo test --lib runtime::registry 2>&1`
Expected: All 6 registry tests pass. The `test_query_params_passed` test sends `10` as a number; Luau produces `Value::Number(10.0)`, our updated `lua_value_to_string` formats it as `"10"`, matching the expected assertion.

**Step 5: Run all runtime tests**

Run: `cargo test --lib runtime 2>&1`
Expected: All sandbox + executor + registry tests pass.

**Step 6: Commit**

```bash
git add src/runtime/registry.rs
git commit -m "feat: add type-informed integer handling for Luau numbers

Use manifest ParamType::Integer to round numbers for integer API
params. Update lua_value_to_string to format whole numbers without
decimal point."
```

---

### Task 4: Rewrite annotations to Luau type syntax

This is the largest task. We rewrite `annotations.rs` to emit Luau native types instead of EmmyLua comments.

**Files:**
- Modify: `src/codegen/annotations.rs` (full rewrite)

**Step 1: Update module doc comment and render_function_annotation**

Replace the doc comment (lines 6-17) and the entire `render_function_annotation` function with:

```rust
/// Render a Luau type-annotated documentation block for a single function.
///
/// Produces output like:
/// ```luau
/// -- Get a pet by ID
/// --
/// -- Returns a single pet by its unique identifier.
/// --
/// -- @param pet_id - The pet's unique identifier
/// function sdk.get_pet(pet_id: string): Pet end
/// ```
pub fn render_function_annotation(func: &FunctionDef) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Summary line
    if let Some(summary) = &func.summary {
        lines.push(format!("-- {}", summary.trim()));
    }

    // Description block (separated by blank comment line)
    if let Some(description) = &func.description {
        let desc = description.trim();
        if !desc.is_empty() {
            lines.push("--".to_string());
            for desc_line in desc.lines() {
                let trimmed = desc_line.trim();
                if trimmed.is_empty() {
                    lines.push("--".to_string());
                } else {
                    lines.push(format!("-- {trimmed}"));
                }
            }
        }
    }

    // Deprecated annotation
    if func.deprecated {
        lines.push("-- @deprecated".to_string());
    }

    // Parameter descriptions as comments (types go in signature)
    for param in &func.parameters {
        if let Some(desc) = &param.description {
            let desc = desc.trim();
            if !desc.is_empty() {
                lines.push(format!("-- @param {} - {desc}", param.name));
            }
        }
    }

    // Request body description
    if let Some(body) = &func.request_body {
        if let Some(desc) = &body.description {
            let desc = desc.trim();
            if !desc.is_empty() {
                lines.push(format!("-- @param body - {desc}"));
            }
        }
    }

    // Function signature with inline types
    let mut typed_params: Vec<String> = func
        .parameters
        .iter()
        .map(|p| {
            let type_str = p.enum_values.as_ref().map_or_else(
                || param_type_to_luau(&p.param_type),
                |ev| render_enum_type(ev),
            );
            if p.required {
                format!("{}: {type_str}", p.name)
            } else {
                format!("{}: {type_str}?", p.name)
            }
        })
        .collect();

    if let Some(body) = &func.request_body {
        if body.required {
            typed_params.push(format!("body: {}", body.schema));
        } else {
            typed_params.push(format!("body: {}?", body.schema));
        }
    }

    let params_str = typed_params.join(", ");
    let return_type = func
        .response_schema
        .as_ref()
        .map_or_else(String::new, |r| format!(": {r}"));

    lines.push(format!(
        "function sdk.{}({params_str}){return_type} end",
        func.name
    ));

    lines.join("\n")
}
```

**Step 2: Rewrite render_schema_annotation**

Replace the doc comment (lines 93-102) and the entire `render_schema_annotation` function with:

```rust
/// Render a Luau `export type` definition for a schema.
///
/// Produces output like:
/// ```luau
/// -- A pet in the store
/// export type Pet = {
///     id: string,              -- Unique ID
///     name: string,            -- The pet's name
///     status: ("available" | "pending" | "sold")?,  -- Current status
/// }
/// ```
pub fn render_schema_annotation(schema: &SchemaDef) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Description line
    if let Some(description) = &schema.description {
        let desc = description.trim();
        if !desc.is_empty() {
            lines.push(format!("-- {desc}"));
        }
    }

    // Type definition opening
    lines.push(format!("export type {} = {{", schema.name));

    // Fields
    for field in &schema.fields {
        let type_str = field.enum_values.as_ref().map_or_else(
            || field_type_to_luau(&field.field_type),
            |ev| render_enum_type(ev),
        );
        let optional_marker = if field.required { "" } else { "?" };
        let desc = field
            .description
            .as_deref()
            .map_or_else(String::new, |d| format!("  -- {}", d.trim()));

        lines.push(format!(
            "    {}: {type_str}{optional_marker},{desc}",
            field.name
        ));
    }

    // Closing brace
    lines.push("}".to_string());

    lines.join("\n")
}
```

**Step 3: Update generate_annotation_files**

In `generate_annotation_files`, change two lines:

Line 208 — change `.lua` to `.luau`:
```rust
        files.push((format!("{tag}.luau"), content));
```

Line 229 — change `_meta.lua` to `_meta.luau`:
```rust
    files.push(("_meta.luau".to_string(), meta_content));
```

**Step 4: Update type conversion helpers**

Rename and update the type conversion functions:

```rust
/// Convert a `ParamType` to its Luau type name.
fn param_type_to_luau(param_type: &ParamType) -> String {
    match param_type {
        ParamType::String => "string".to_string(),
        ParamType::Integer | ParamType::Number => "number".to_string(),
        ParamType::Boolean => "boolean".to_string(),
    }
}

/// Convert a `FieldType` to its Luau type name.
fn field_type_to_luau(field_type: &FieldType) -> String {
    match field_type {
        FieldType::String => "string".to_string(),
        FieldType::Integer | FieldType::Number => "number".to_string(),
        FieldType::Boolean => "boolean".to_string(),
        FieldType::Array { items } => format!("{{{}}}", field_type_to_luau(items)),
        FieldType::Object { schema } => schema.clone(),
    }
}
```

Note the key differences from the Lua 5.4 versions:
- `Integer` maps to `"number"` (not `"integer"` — Luau has no integer type)
- Arrays use `{string}` syntax instead of `string[]`

**Step 5: Update render_enum_type**

Replace `render_enum_type` to use Luau union syntax with spaces around `|`:

```rust
/// Render an enum type as a Luau literal union: `"val1" | "val2" | "val3"`.
fn render_enum_type(values: &[String]) -> String {
    let inner = values
        .iter()
        .map(|v| format!("\"{v}\""))
        .collect::<Vec<_>>()
        .join(" | ");
    format!("({inner})")
}
```

Note: Wraps in parentheses for use in both params (`status: ("a" | "b")?`) and fields (`status: ("a" | "b")?,`).

**Step 6: Rewrite all tests**

Replace the entire `#[cfg(test)] mod tests` block with tests matching the new Luau output format:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::codegen::manifest::*;

    #[test]
    fn test_render_function_annotation() {
        let func = FunctionDef {
            name: "get_pet".to_string(),
            api: "petstore".to_string(),
            tag: Some("pets".to_string()),
            method: HttpMethod::Get,
            path: "/pets/{pet_id}".to_string(),
            summary: Some("Get a pet by ID".to_string()),
            description: Some("Returns a single pet by its unique identifier.".to_string()),
            deprecated: false,
            parameters: vec![ParamDef {
                name: "pet_id".to_string(),
                location: ParamLocation::Path,
                param_type: ParamType::String,
                required: true,
                description: Some("The pet's unique identifier".to_string()),
                default: None,
                enum_values: None,
            }],
            request_body: None,
            response_schema: Some("Pet".to_string()),
        };

        let output = render_function_annotation(&func);
        assert!(output.contains("-- Get a pet by ID"), "Missing summary. Got:\n{output}");
        assert!(
            output.contains("-- Returns a single pet by its unique identifier."),
            "Missing description. Got:\n{output}"
        );
        assert!(
            output.contains("-- @param pet_id - The pet's unique identifier"),
            "Missing @param. Got:\n{output}"
        );
        assert!(
            output.contains("function sdk.get_pet(pet_id: string): Pet end"),
            "Missing typed function signature. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_function_with_optional_params() {
        let func = FunctionDef {
            name: "list_pets".to_string(),
            api: "petstore".to_string(),
            tag: Some("pets".to_string()),
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
                },
                ParamDef {
                    name: "limit".to_string(),
                    location: ParamLocation::Query,
                    param_type: ParamType::Integer,
                    required: true,
                    description: Some("Max items".to_string()),
                    default: None,
                    enum_values: None,
                },
            ],
            request_body: None,
            response_schema: Some("Pet".to_string()),
        };

        let output = render_function_annotation(&func);
        // Optional param gets ? after type
        assert!(
            output.contains("status: string?"),
            "Optional param missing ? suffix. Got:\n{output}"
        );
        // Required param — no ?; integer becomes number
        assert!(
            output.contains("limit: number"),
            "Required param should use number type. Got:\n{output}"
        );
        assert!(
            !output.contains("limit: number?"),
            "Required param should NOT have ?. Got:\n{output}"
        );
        assert!(
            output.contains("function sdk.list_pets(status: string?, limit: number): Pet end"),
            "Missing typed function signature. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_function_with_enum_param() {
        let func = FunctionDef {
            name: "list_pets".to_string(),
            api: "petstore".to_string(),
            tag: None,
            method: HttpMethod::Get,
            path: "/pets".to_string(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: vec![ParamDef {
                name: "status".to_string(),
                location: ParamLocation::Query,
                param_type: ParamType::String,
                required: false,
                description: Some("Filter by status".to_string()),
                default: None,
                enum_values: Some(vec![
                    "available".to_string(),
                    "pending".to_string(),
                    "sold".to_string(),
                ]),
            }],
            request_body: None,
            response_schema: None,
        };

        let output = render_function_annotation(&func);
        assert!(
            output.contains(r#"status: ("available" | "pending" | "sold")?"#),
            "Enum param should use Luau union type. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_function_deprecated() {
        let func = FunctionDef {
            name: "old_endpoint".to_string(),
            api: "myapi".to_string(),
            tag: None,
            method: HttpMethod::Get,
            path: "/old".to_string(),
            summary: Some("An old endpoint".to_string()),
            description: None,
            deprecated: true,
            parameters: vec![],
            request_body: None,
            response_schema: None,
        };

        let output = render_function_annotation(&func);
        assert!(
            output.contains("-- @deprecated"),
            "Missing @deprecated annotation. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_function_with_request_body() {
        let func = FunctionDef {
            name: "create_pet".to_string(),
            api: "petstore".to_string(),
            tag: Some("pets".to_string()),
            method: HttpMethod::Post,
            path: "/pets".to_string(),
            summary: Some("Create a new pet".to_string()),
            description: None,
            deprecated: false,
            parameters: vec![],
            request_body: Some(RequestBodyDef {
                content_type: "application/json".to_string(),
                schema: "NewPet".to_string(),
                required: true,
                description: Some("The pet to create".to_string()),
            }),
            response_schema: Some("Pet".to_string()),
        };

        let output = render_function_annotation(&func);
        assert!(
            output.contains("-- @param body - The pet to create"),
            "Missing body param description. Got:\n{output}"
        );
        assert!(
            output.contains("function sdk.create_pet(body: NewPet): Pet end"),
            "Missing typed body in function signature. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_schema_annotation() {
        let schema = SchemaDef {
            name: "Pet".to_string(),
            description: Some("A pet in the store".to_string()),
            fields: vec![
                FieldDef {
                    name: "id".to_string(),
                    field_type: FieldType::String,
                    required: true,
                    description: Some("Unique ID".to_string()),
                    enum_values: None,
                },
                FieldDef {
                    name: "name".to_string(),
                    field_type: FieldType::String,
                    required: true,
                    description: Some("The pet's name".to_string()),
                    enum_values: None,
                },
                FieldDef {
                    name: "tags".to_string(),
                    field_type: FieldType::Array {
                        items: Box::new(FieldType::String),
                    },
                    required: false,
                    description: Some("Classification tags".to_string()),
                    enum_values: None,
                },
                FieldDef {
                    name: "owner".to_string(),
                    field_type: FieldType::Object {
                        schema: "User".to_string(),
                    },
                    required: false,
                    description: Some("The pet's owner".to_string()),
                    enum_values: None,
                },
            ],
        };

        let output = render_schema_annotation(&schema);
        assert!(
            output.contains("-- A pet in the store"),
            "Missing description. Got:\n{output}"
        );
        assert!(
            output.contains("export type Pet = {"),
            "Missing export type. Got:\n{output}"
        );
        assert!(
            output.contains("    id: string,  -- Unique ID"),
            "Missing id field. Got:\n{output}"
        );
        assert!(
            output.contains("    name: string,  -- The pet's name"),
            "Missing name field. Got:\n{output}"
        );
        assert!(
            output.contains("    tags: {string}?,  -- Classification tags"),
            "Missing array field with Luau syntax. Got:\n{output}"
        );
        assert!(
            output.contains("    owner: User?,  -- The pet's owner"),
            "Missing object field. Got:\n{output}"
        );
        assert!(output.contains("}"), "Missing closing brace. Got:\n{output}");
    }

    #[test]
    fn test_render_schema_optional_fields() {
        let schema = SchemaDef {
            name: "Item".to_string(),
            description: None,
            fields: vec![
                FieldDef {
                    name: "id".to_string(),
                    field_type: FieldType::String,
                    required: true,
                    description: None,
                    enum_values: None,
                },
                FieldDef {
                    name: "label".to_string(),
                    field_type: FieldType::String,
                    required: false,
                    description: None,
                    enum_values: None,
                },
            ],
        };

        let output = render_schema_annotation(&schema);
        // Required field: no ?
        assert!(
            output.contains("    id: string,"),
            "Required field should not have ?. Got:\n{output}"
        );
        assert!(
            !output.contains("id: string?,"),
            "Required field should NOT have ?. Got:\n{output}"
        );
        // Optional field: has ?
        assert!(
            output.contains("    label: string?,"),
            "Optional field missing ? suffix. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_schema_enum_fields() {
        let schema = SchemaDef {
            name: "Pet".to_string(),
            description: None,
            fields: vec![FieldDef {
                name: "status".to_string(),
                field_type: FieldType::String,
                required: true,
                description: Some("Current status".to_string()),
                enum_values: Some(vec![
                    "available".to_string(),
                    "pending".to_string(),
                    "sold".to_string(),
                ]),
            }],
        };

        let output = render_schema_annotation(&schema);
        assert!(
            output.contains(r#"    status: ("available" | "pending" | "sold"),  -- Current status"#),
            "Enum field should use Luau union type. Got:\n{output}"
        );
    }

    #[test]
    fn test_generate_annotation_files() {
        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "petstore".to_string(),
                base_url: "https://petstore.example.com/v1".to_string(),
                description: Some("A sample petstore API".to_string()),
                version: Some("1.0.0".to_string()),
                auth: Some(AuthConfig::Bearer {
                    header: "Authorization".to_string(),
                    prefix: "Bearer ".to_string(),
                }),
            }],
            functions: vec![
                FunctionDef {
                    name: "list_pets".to_string(),
                    api: "petstore".to_string(),
                    tag: Some("pets".to_string()),
                    method: HttpMethod::Get,
                    path: "/pets".to_string(),
                    summary: Some("List all pets".to_string()),
                    description: None,
                    deprecated: false,
                    parameters: vec![],
                    request_body: None,
                    response_schema: Some("Pet".to_string()),
                },
                FunctionDef {
                    name: "create_pet".to_string(),
                    api: "petstore".to_string(),
                    tag: Some("pets".to_string()),
                    method: HttpMethod::Post,
                    path: "/pets".to_string(),
                    summary: Some("Create a pet".to_string()),
                    description: None,
                    deprecated: false,
                    parameters: vec![],
                    request_body: Some(RequestBodyDef {
                        content_type: "application/json".to_string(),
                        schema: "NewPet".to_string(),
                        required: true,
                        description: None,
                    }),
                    response_schema: Some("Pet".to_string()),
                },
            ],
            schemas: vec![
                SchemaDef {
                    name: "Pet".to_string(),
                    description: Some("A pet in the store".to_string()),
                    fields: vec![FieldDef {
                        name: "id".to_string(),
                        field_type: FieldType::String,
                        required: true,
                        description: Some("Unique ID".to_string()),
                        enum_values: None,
                    }],
                },
                SchemaDef {
                    name: "NewPet".to_string(),
                    description: Some("Data for a new pet".to_string()),
                    fields: vec![FieldDef {
                        name: "name".to_string(),
                        field_type: FieldType::String,
                        required: true,
                        description: Some("Pet name".to_string()),
                        enum_values: None,
                    }],
                },
            ],
        };

        let files = generate_annotation_files(&manifest);

        // Should have: pets.luau + _meta.luau
        assert!(
            files.len() >= 2,
            "Expected at least 2 files, got {}",
            files.len()
        );

        // All filenames should end in .luau
        for (filename, _) in &files {
            assert!(
                filename.ends_with(".luau"),
                "File {filename} doesn't end in .luau"
            );
        }

        // Check pets.luau exists and has content
        let pets_file = files.iter().find(|(name, _)| name == "pets.luau");
        assert!(pets_file.is_some(), "Missing pets.luau");
        let pets_content = &pets_file.unwrap().1;
        assert!(!pets_content.is_empty(), "pets.luau is empty");
        assert!(
            pets_content.contains("function sdk.list_pets"),
            "pets.luau missing list_pets function"
        );
        assert!(
            pets_content.contains("function sdk.create_pet"),
            "pets.luau missing create_pet function"
        );
        assert!(
            pets_content.contains("export type Pet"),
            "pets.luau missing Pet type"
        );
        assert!(
            pets_content.contains("export type NewPet"),
            "pets.luau missing NewPet type"
        );

        // Check _meta.luau exists
        let meta_file = files.iter().find(|(name, _)| name == "_meta.luau");
        assert!(meta_file.is_some(), "Missing _meta.luau");
        let meta_content = &meta_file.unwrap().1;
        assert!(
            meta_content.contains("petstore"),
            "_meta.luau missing API name"
        );
        assert!(meta_content.contains("1.0.0"), "_meta.luau missing version");
    }
}
```

**Step 7: Run annotation tests**

Run: `cargo test --lib codegen::annotations 2>&1`
Expected: All tests pass.

**Step 8: Commit**

```bash
git add src/codegen/annotations.rs
git commit -m "feat: rewrite annotations from EmmyLua to Luau native types

Replace comment-based EmmyLua annotations (--- @class, --- @field,
--- @param) with Luau native type syntax (export type, inline param
types, {array} syntax). Change file extensions from .lua to .luau."
```

---

### Task 5: Update generate.rs and its tests

**Files:**
- Modify: `src/codegen/generate.rs:107-118`

**Step 1: Update the generate test**

In `test_generate_creates_output`, change `.lua` references to `.luau`:

Replace lines 107-118 with:
```rust
        // Should have at least one .luau file
        let luau_files: Vec<_> = std::fs::read_dir(&sdk_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "luau")
                    .unwrap_or(false)
            })
            .collect();
        assert!(!luau_files.is_empty(), "No .luau files in sdk/");
```

**Step 2: Run generate tests**

Run: `cargo test --lib codegen::generate 2>&1`
Expected: All tests pass.

**Step 3: Commit**

```bash
git add src/codegen/generate.rs
git commit -m "fix: update generate test to expect .luau file extension"
```

---

### Task 6: Update tool descriptions for Luau

**Files:**
- Modify: `src/server/tools.rs:92,178,244,307,356,460,522`

**Step 1: Update tool description strings**

These are cosmetic but important for clarity. Update the following strings:

Line 244/460 — `get_function_docs` tool description:
```
"Get the full Luau type annotation documentation for a specific function"
```

Line 307/522 — `get_schema` tool description:
```
"Get the full Luau type annotation documentation for a schema (class/type)"
```

Line 356 — `execute_script` tool description:
```
"Execute a Luau script against the SDK. Auth comes from server-side configuration."
```

Line 360 — `execute_script` schema, `script` property description:
```
"description": "Luau script to execute"
```

Line 92 — `get_function_docs_impl` doc comment:
```
/// Implementation for `get_function_docs`: returns the full Luau type annotation.
```

Line 178 — `get_schema_impl` doc comment:
```
/// Implementation for `get_schema`: returns the full Luau type annotation for a schema.
```

**Step 2: Run full test suite**

Run: `cargo test 2>&1`
Expected: All tests pass.

**Step 3: Commit**

```bash
git add src/server/tools.rs
git commit -m "docs: update tool descriptions from Lua/LuaLS to Luau"
```

---

### Task 7: Final verification

**Step 1: Run the full test suite**

Run: `cargo test 2>&1`
Expected: All tests pass with no warnings.

**Step 2: Run clippy**

Run: `cargo clippy 2>&1`
Expected: No errors, no new warnings.

**Step 3: Run the generate command against petstore**

Run: `cargo run -- generate testdata/petstore.yaml --output /tmp/test-luau-gen 2>&1`
Then check the output: `ls /tmp/test-luau-gen/sdk/` — should show `.luau` files.
Read one: `cat /tmp/test-luau-gen/sdk/*.luau | head -30` — should show `export type`, typed function signatures.

**Step 4: Commit any final adjustments**

If any small fixes are needed from the verification, commit them.
