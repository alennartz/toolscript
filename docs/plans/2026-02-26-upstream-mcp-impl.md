# Upstream MCP Server Support Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let Luau scripts call tools on upstream MCP servers alongside OpenAPI functions, under a unified `sdk.*` namespace.

**Architecture:** ToolScript connects to upstream MCP servers as a client at startup, discovers their tools, converts JSON Schema to Luau type annotations using shared utilities, and registers Luau closures backed by `call_tool`. The inline object bug is fixed first since both OpenAPI and MCP depend on correct nested type rendering.

**Tech Stack:** Rust, rmcp 0.16 (client + server), mlua/Luau, serde_json, TOML config

---

### Task 1: Fix Inline Object Bug — Extend FieldType

**Files:**
- Modify: `src/codegen/manifest.rs:127-135`
- Test: `src/codegen/manifest.rs` (existing test module)

**Step 1: Write the failing test**

Add to the existing `mod tests` block in `src/codegen/manifest.rs`:

```rust
#[test]
fn test_field_type_inline_object_serde() {
    let inline = FieldType::InlineObject {
        fields: vec![
            FieldDef {
                name: "timeout".to_string(),
                field_type: FieldType::Integer,
                required: true,
                description: Some("Timeout in ms".to_string()),
                enum_values: None,
                nullable: false,
                format: None,
            },
            FieldDef {
                name: "retries".to_string(),
                field_type: FieldType::Number,
                required: false,
                description: None,
                enum_values: None,
                nullable: false,
                format: None,
            },
        ],
    };
    let json = serde_json::to_string(&inline).unwrap();
    let deserialized: FieldType = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, inline);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib codegen::manifest::tests::test_field_type_inline_object_serde`
Expected: FAIL — `InlineObject` variant does not exist

**Step 3: Write minimal implementation**

Add the `InlineObject` variant to `FieldType` in `src/codegen/manifest.rs:127-135`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldType {
    String,
    Integer,
    Number,
    Boolean,
    Array { items: Box<Self> },
    Object { schema: String },
    InlineObject { fields: Vec<FieldDef> },
    Map { value: Box<Self> },
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib codegen::manifest::tests::test_field_type_inline_object_serde`
Expected: PASS

**Step 5: Commit**

```bash
git add src/codegen/manifest.rs
git commit -m "feat: add InlineObject variant to FieldType"
```

---

### Task 2: Fix Inline Object Bug — Render Luau Annotations

**Files:**
- Modify: `src/codegen/annotations.rs:296-304` (`field_type_to_luau`)
- Test: `src/codegen/annotations.rs` (existing test module)

**Step 1: Write the failing test**

Add to `mod tests` in `src/codegen/annotations.rs`:

```rust
#[test]
fn test_render_inline_object_field() {
    let schema = SchemaDef {
        name: "Config".to_string(),
        description: None,
        fields: vec![FieldDef {
            name: "options".to_string(),
            field_type: FieldType::InlineObject {
                fields: vec![
                    FieldDef {
                        name: "timeout".to_string(),
                        field_type: FieldType::Integer,
                        required: true,
                        description: Some("Timeout in ms".to_string()),
                        enum_values: None,
                        nullable: false,
                        format: None,
                    },
                    FieldDef {
                        name: "retries".to_string(),
                        field_type: FieldType::Number,
                        required: false,
                        description: None,
                        enum_values: None,
                        nullable: false,
                        format: None,
                    },
                ],
            },
            required: true,
            description: None,
            enum_values: None,
            nullable: false,
            format: None,
        }],
    };

    let output = render_schema_annotation(&schema);
    assert!(
        output.contains("options: { timeout: number, retries: number? },"),
        "Inline object should render nested fields. Got:\n{output}"
    );
}

#[test]
fn test_render_deeply_nested_inline_object() {
    let schema = SchemaDef {
        name: "Root".to_string(),
        description: None,
        fields: vec![FieldDef {
            name: "outer".to_string(),
            field_type: FieldType::InlineObject {
                fields: vec![FieldDef {
                    name: "inner".to_string(),
                    field_type: FieldType::InlineObject {
                        fields: vec![FieldDef {
                            name: "value".to_string(),
                            field_type: FieldType::String,
                            required: true,
                            description: None,
                            enum_values: None,
                            nullable: false,
                            format: None,
                        }],
                    },
                    required: true,
                    description: None,
                    enum_values: None,
                    nullable: false,
                    format: None,
                }],
            },
            required: true,
            description: None,
            enum_values: None,
            nullable: false,
            format: None,
        }],
    };

    let output = render_schema_annotation(&schema);
    assert!(
        output.contains("outer: { inner: { value: string } },"),
        "Deeply nested inline objects should render correctly. Got:\n{output}"
    );
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib codegen::annotations::tests::test_render_inline_object_field codegen::annotations::tests::test_render_deeply_nested_inline_object`
Expected: FAIL — no match arm for `InlineObject`

**Step 3: Write minimal implementation**

Update `field_type_to_luau` in `src/codegen/annotations.rs`:

```rust
fn field_type_to_luau(field_type: &FieldType) -> String {
    match field_type {
        FieldType::String => "string".to_string(),
        FieldType::Integer | FieldType::Number => "number".to_string(),
        FieldType::Boolean => "boolean".to_string(),
        FieldType::Array { items } => format!("{{{}}}", field_type_to_luau(items)),
        FieldType::Object { schema } => schema.clone(),
        FieldType::InlineObject { fields } => {
            let entries: Vec<String> = fields
                .iter()
                .map(|f| {
                    let type_str = f.enum_values.as_ref().map_or_else(
                        || field_type_to_luau(&f.field_type),
                        |ev| render_enum_type(ev),
                    );
                    let optional = if !f.required || f.nullable { "?" } else { "" };
                    format!("{}: {type_str}{optional}", f.name)
                })
                .collect();
            format!("{{ {} }}", entries.join(", "))
        }
        FieldType::Map { value } => format!("{{ [string]: {} }}", field_type_to_luau(value)),
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib codegen::annotations::tests`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add src/codegen/annotations.rs
git commit -m "fix: render InlineObject as nested Luau types instead of 'unknown'"
```

---

### Task 3: Fix Inline Object Bug — Parser

**Files:**
- Modify: `src/codegen/parser.rs:597-633` (`schema_kind_to_field_type`)
- Test: `src/codegen/parser.rs` or `tests/` integration

**Step 1: Write the failing test**

Add to `mod tests` in `src/codegen/parser.rs` (or the test that already parses schemas):

```rust
#[test]
fn test_inline_object_parsed_as_inline_object() {
    let spec_json = serde_json::json!({
        "openapi": "3.0.0",
        "info": { "title": "Test", "version": "1.0" },
        "servers": [{ "url": "https://api.example.com" }],
        "paths": {},
        "components": {
            "schemas": {
                "Config": {
                    "type": "object",
                    "required": ["settings"],
                    "properties": {
                        "settings": {
                            "type": "object",
                            "required": ["timeout"],
                            "properties": {
                                "timeout": { "type": "integer" },
                                "retries": { "type": "integer" }
                            }
                        }
                    }
                }
            }
        }
    });
    let spec: OpenAPI = serde_json::from_value(spec_json).unwrap();
    let manifest = parse_spec("test", &spec).unwrap();
    let config_schema = manifest.schemas.iter().find(|s| s.name == "Config").unwrap();
    let settings_field = config_schema.fields.iter().find(|f| f.name == "settings").unwrap();
    match &settings_field.field_type {
        FieldType::InlineObject { fields } => {
            assert!(fields.iter().any(|f| f.name == "timeout"));
            assert!(fields.iter().any(|f| f.name == "retries"));
        }
        other => panic!("Expected InlineObject, got {other:?}"),
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib codegen::parser::tests::test_inline_object_parsed_as_inline_object`
Expected: FAIL — currently returns `Object { schema: "unknown" }`

**Step 3: Write minimal implementation**

Update `schema_kind_to_field_type` in `src/codegen/parser.rs:621-629`. Replace the Object arm:

```rust
SchemaKind::Type(Type::Object(obj)) => {
    if obj.properties.is_empty() {
        if let Some(ap) = &obj.additional_properties {
            return additional_properties_to_map(ap);
        }
        // Empty object with no additional_properties
        return FieldType::Map {
            value: Box::new(FieldType::String),
        };
    }
    // Has properties — build inline object
    let required_set: std::collections::HashSet<&str> =
        obj.required.iter().map(String::as_str).collect();
    let fields: Vec<FieldDef> = obj
        .properties
        .iter()
        .map(|(name, schema_ref)| {
            let is_required = required_set.contains(name.as_str());
            match schema_ref {
                ReferenceOr::Reference { reference } => {
                    let schema_name = reference
                        .strip_prefix("#/components/schemas/")
                        .unwrap_or(reference);
                    FieldDef {
                        name: name.clone(),
                        field_type: FieldType::Object { schema: schema_name.to_string() },
                        required: is_required,
                        description: None,
                        enum_values: None,
                        nullable: false,
                        format: None,
                    }
                }
                ReferenceOr::Item(schema) => {
                    let field_type = schema_kind_to_field_type(&schema.schema_kind);
                    let enum_values = extract_field_enum_values(&schema.schema_kind);
                    let nullable = schema.schema_data.nullable;
                    let format = extract_format(&schema.schema_kind);
                    FieldDef {
                        name: name.clone(),
                        field_type,
                        required: is_required,
                        description: schema.schema_data.description.clone(),
                        enum_values,
                        nullable,
                        format,
                    }
                }
            }
        })
        .collect();
    FieldType::InlineObject { fields }
}
```

Note: This calls `schema_kind_to_field_type` recursively for nested inline objects.

**Step 4: Run tests to verify all pass**

Run: `cargo test --lib codegen::parser::tests`
Expected: ALL PASS (new test + existing tests)

Run: `cargo test` (full suite to check no regressions)
Expected: ALL PASS

**Step 5: Commit**

```bash
git add src/codegen/parser.rs
git commit -m "fix: parse inline objects recursively instead of emitting 'unknown'"
```

---

### Task 4: Enhance get_function_docs with Referenced Schemas

**Files:**
- Modify: `src/server/mod.rs:46-50` (annotation cache building)
- Modify: `src/codegen/annotations.rs` (add `render_function_docs` that includes schemas)
- Modify: `src/server/tools.rs:97-103` (`get_function_docs_impl`)
- Test: `src/server/mod.rs` tests

**Step 1: Write the failing test**

Add to `mod tests` in `src/server/mod.rs`:

```rust
#[test]
fn test_get_function_docs_includes_referenced_schemas() {
    let server = test_server();
    let docs = tools::get_function_docs_impl(&server, "create_pet").unwrap();
    // create_pet has request_body: NewPet and response: Pet
    assert!(docs.contains("function sdk.create_pet"), "Missing function sig. Got:\n{docs}");
    assert!(docs.contains("export type NewPet"), "Missing NewPet schema. Got:\n{docs}");
    assert!(docs.contains("export type Pet"), "Missing Pet schema. Got:\n{docs}");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib server::tests::test_get_function_docs_includes_referenced_schemas`
Expected: FAIL — current annotation cache only has the signature, not schemas

**Step 3: Write minimal implementation**

In `src/codegen/annotations.rs`, add a new function:

```rust
/// Render a complete documentation block for a function: signature + all referenced schemas.
///
/// Walks the function's response_schema and request_body to collect referenced types,
/// then transitively collects schemas those types reference, deduplicates, and appends
/// them as `export type` blocks after the function signature.
pub fn render_function_docs(func: &FunctionDef, schemas: &[SchemaDef]) -> String {
    let mut output = render_function_annotation(func);

    // Collect directly referenced schema names
    let mut needed: Vec<String> = Vec::new();
    if let Some(ref schema) = func.response_schema {
        needed.push(schema.clone());
    }
    if let Some(ref body) = func.request_body {
        needed.push(body.schema.clone());
    }

    // Build schema lookup
    let schema_map: std::collections::HashMap<&str, &SchemaDef> =
        schemas.iter().map(|s| (s.name.as_str(), s)).collect();

    // Transitively collect all referenced schemas
    let mut resolved: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue = needed;
    while let Some(name) = queue.pop() {
        if !resolved.insert(name.clone()) {
            continue;
        }
        if let Some(schema) = schema_map.get(name.as_str()) {
            for field in &schema.fields {
                collect_type_refs(&field.field_type, &mut queue);
            }
        }
    }

    // Render in stable order
    let mut sorted: Vec<&str> = resolved.iter().map(String::as_str).collect();
    sorted.sort();
    for name in sorted {
        if let Some(schema) = schema_map.get(name) {
            output.push('\n');
            output.push('\n');
            output.push_str(&render_schema_annotation(schema));
        }
    }

    output
}

/// Collect named type references from a FieldType.
fn collect_type_refs(field_type: &FieldType, refs: &mut Vec<String>) {
    match field_type {
        FieldType::Object { schema } => refs.push(schema.clone()),
        FieldType::Array { items } => collect_type_refs(items, refs),
        FieldType::InlineObject { fields } => {
            for f in fields {
                collect_type_refs(&f.field_type, refs);
            }
        }
        FieldType::Map { value } => collect_type_refs(value, refs),
        _ => {}
    }
}
```

Update `src/server/mod.rs:46-50` to use `render_function_docs`:

```rust
use crate::codegen::annotations::{render_function_docs, render_schema_annotation};

// In ToolScriptServer::new():
let annotation_cache: HashMap<String, String> = manifest
    .functions
    .iter()
    .map(|f| (f.name.clone(), render_function_docs(f, &manifest.schemas)))
    .collect();
```

**Step 4: Run tests**

Run: `cargo test --lib server::tests`
Expected: ALL PASS (new test + existing)

**Step 5: Commit**

```bash
git add src/codegen/annotations.rs src/server/mod.rs
git commit -m "feat: get_function_docs includes transitively referenced schemas"
```

---

### Task 5: Remove get_schema Tool

**Files:**
- Modify: `src/server/mod.rs:23-34,52-56,123-131,157-163`
- Modify: `src/server/tools.rs:186-192,310-339,532-561`
- Modify: `src/server/resources.rs` (remove schema_cache parameter from `read_resource`)
- Modify: `src/main.rs:397-402` (HTTP tool registration)
- Test: `src/server/mod.rs` tests — remove `test_get_schema_*` tests

**Step 1: Remove `schema_cache` from `ToolScriptServer`**

Remove the `schema_cache` field and its construction. Remove `get_schema_tool()` and `get_schema_tool_arc()` from tool registration in `into_router()` and the HTTP serve path. Remove `get_schema_impl`. Remove the `test_get_schema_found` and `test_get_schema_not_found` tests. Update `read_resource` calls to not pass `schema_cache`. Keep `render_schema_annotation` — it's still used by `render_function_docs`.

**Step 2: Run tests**

Run: `cargo test`
Expected: ALL PASS

**Step 3: Commit**

```bash
git add src/server/mod.rs src/server/tools.rs src/server/resources.rs src/main.rs
git commit -m "feat: remove get_schema tool, absorbed into get_function_docs"
```

---

### Task 6: Add MCP Server Config Types

**Files:**
- Modify: `src/config.rs:50-57` (`ToolScriptConfig`)
- Test: `src/config.rs` tests

**Step 1: Write the failing test**

Add to `mod tests` in `src/config.rs`:

```rust
#[test]
fn test_load_config_with_mcp_servers() {
    let toml_content = r#"
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

[mcp_servers.remote]
url = "https://mcp.example.com/mcp"

[mcp_servers.legacy]
url = "https://mcp.example.com/sse"
transport = "sse"
"#;
    let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
    tmpfile.write_all(toml_content.as_bytes()).unwrap();

    let config = load_config(tmpfile.path()).unwrap();
    let mcp = config.mcp_servers.as_ref().unwrap();
    assert_eq!(mcp.len(), 3);

    let fs = &mcp["filesystem"];
    assert_eq!(fs.command.as_deref(), Some("npx"));
    assert_eq!(fs.args.as_ref().unwrap().len(), 3);
    assert!(fs.url.is_none());
    assert!(fs.transport.is_none());

    let remote = &mcp["remote"];
    assert_eq!(remote.url.as_deref(), Some("https://mcp.example.com/mcp"));
    assert!(remote.command.is_none());
    assert!(remote.transport.is_none()); // defaults to streamable-http

    let legacy = &mcp["legacy"];
    assert_eq!(legacy.transport.as_deref(), Some("sse"));
}

#[test]
fn test_load_config_mcp_only() {
    let toml_content = r#"
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
"#;
    let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
    tmpfile.write_all(toml_content.as_bytes()).unwrap();

    let config = load_config(tmpfile.path()).unwrap();
    assert!(config.apis.is_empty());
    assert!(config.mcp_servers.as_ref().unwrap().len() == 1);
}

#[test]
fn test_validate_mcp_server_config() {
    // command + url = error
    let entry = McpServerConfigEntry {
        command: Some("npx".to_string()),
        args: None,
        env: None,
        url: Some("https://example.com".to_string()),
        transport: None,
    };
    assert!(validate_mcp_server_entry("test", &entry).is_err());

    // neither = error
    let entry = McpServerConfigEntry {
        command: None,
        args: None,
        env: None,
        url: None,
        transport: None,
    };
    assert!(validate_mcp_server_entry("test", &entry).is_err());

    // transport with command = error
    let entry = McpServerConfigEntry {
        command: Some("npx".to_string()),
        args: None,
        env: None,
        url: None,
        transport: Some("sse".to_string()),
    };
    assert!(validate_mcp_server_entry("test", &entry).is_err());

    // args with url = error
    let entry = McpServerConfigEntry {
        command: None,
        args: Some(vec!["foo".to_string()]),
        env: None,
        url: Some("https://example.com".to_string()),
        transport: None,
    };
    assert!(validate_mcp_server_entry("test", &entry).is_err());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib config::tests::test_load_config_with_mcp_servers`
Expected: FAIL — types don't exist

**Step 3: Write minimal implementation**

Add to `src/config.rs`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfigEntry {
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    pub url: Option<String>,
    #[serde(default)]
    pub transport: Option<String>,
}

/// Validate an MCP server config entry.
pub fn validate_mcp_server_entry(name: &str, entry: &McpServerConfigEntry) -> anyhow::Result<()> {
    match (&entry.command, &entry.url) {
        (Some(_), Some(_)) => anyhow::bail!("mcp_servers.{name}: cannot set both 'command' and 'url'"),
        (None, None) => anyhow::bail!("mcp_servers.{name}: must set either 'command' or 'url'"),
        (Some(_), None) => {
            if entry.transport.is_some() {
                anyhow::bail!("mcp_servers.{name}: 'transport' is only valid with 'url', not 'command'");
            }
        }
        (None, Some(_)) => {
            if entry.args.is_some() {
                anyhow::bail!("mcp_servers.{name}: 'args' is only valid with 'command', not 'url'");
            }
            if entry.env.is_some() {
                anyhow::bail!("mcp_servers.{name}: 'env' is only valid with 'command', not 'url'");
            }
            if let Some(ref t) = entry.transport {
                if t != "sse" && t != "streamable-http" {
                    anyhow::bail!("mcp_servers.{name}: transport must be 'sse' or 'streamable-http', got '{t}'");
                }
            }
        }
    }
    Ok(())
}
```

Update `ToolScriptConfig`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ToolScriptConfig {
    #[serde(default)]
    pub apis: HashMap<String, ConfigApiEntry>,
    #[serde(default)]
    pub frozen_params: Option<HashMap<String, String>>,
    #[serde(default)]
    pub output: Option<OutputConfig>,
    #[serde(default)]
    pub mcp_servers: Option<HashMap<String, McpServerConfigEntry>>,
}
```

Note: `apis` gets `#[serde(default)]` so MCP-only configs work.

**Step 4: Run tests**

Run: `cargo test --lib config::tests`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: add MCP server config types and validation"
```

---

### Task 7: Add MCP Manifest Types

**Files:**
- Modify: `src/codegen/manifest.rs:5-10` (`Manifest`)
- Test: `src/codegen/manifest.rs` tests

**Step 1: Write the failing test**

```rust
#[test]
fn test_mcp_server_entry_roundtrip() {
    let entry = McpServerEntry {
        name: "filesystem".to_string(),
        description: Some("File system access".to_string()),
        tools: vec![McpToolDef {
            name: "read_file".to_string(),
            server: "filesystem".to_string(),
            description: Some("Read a file".to_string()),
            params: vec![McpParamDef {
                name: "path".to_string(),
                luau_type: "string".to_string(),
                required: true,
                description: Some("File path to read".to_string()),
            }],
            schemas: vec![],
            output_schemas: vec![],
        }],
    };
    let json = serde_json::to_string(&entry).unwrap();
    let roundtripped: McpServerEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtripped.name, "filesystem");
    assert_eq!(roundtripped.tools.len(), 1);
    assert_eq!(roundtripped.tools[0].params[0].luau_type, "string");
}

#[test]
fn test_manifest_with_mcp_servers_roundtrip() {
    let manifest = Manifest {
        apis: vec![],
        functions: vec![],
        schemas: vec![],
        mcp_servers: vec![McpServerEntry {
            name: "test".to_string(),
            description: None,
            tools: vec![],
        }],
    };
    let json = serde_json::to_string(&manifest).unwrap();
    let roundtripped: Manifest = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtripped.mcp_servers.len(), 1);
}
```

**Step 2: Run tests to verify they fail**

Expected: FAIL — types don't exist

**Step 3: Write minimal implementation**

Add to `src/codegen/manifest.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerEntry {
    pub name: String,
    pub description: Option<String>,
    pub tools: Vec<McpToolDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolDef {
    pub name: String,
    pub server: String,
    pub description: Option<String>,
    pub params: Vec<McpParamDef>,
    #[serde(default)]
    pub schemas: Vec<SchemaDef>,
    #[serde(default)]
    pub output_schemas: Vec<SchemaDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpParamDef {
    pub name: String,
    pub luau_type: String,
    pub required: bool,
    pub description: Option<String>,
}
```

Update `Manifest`:

```rust
pub struct Manifest {
    pub apis: Vec<ApiConfig>,
    pub functions: Vec<FunctionDef>,
    pub schemas: Vec<SchemaDef>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerEntry>,
}
```

**Step 4: Fix all existing `Manifest` constructors in tests to include `mcp_servers: vec![]`**

Search the codebase for all `Manifest {` constructors and add the new field.

**Step 5: Run tests**

Run: `cargo test`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add src/codegen/manifest.rs
git commit -m "feat: add McpServerEntry, McpToolDef, McpParamDef manifest types"
```

---

### Task 8: Add Cargo.toml Client Features + CLI --mcp Flag

**Files:**
- Modify: `Cargo.toml:15` (rmcp features)
- Modify: `src/cli.rs` (add --mcp flag to Run and Serve)
- Modify: `src/config.rs` (add `parse_mcp_arg`)
- Test: `src/config.rs` tests

**Step 1: Update Cargo.toml**

```toml
rmcp = { version = "0.16", features = [
    "server", "client",
    "transport-io", "transport-child-process",
    "transport-streamable-http-server", "transport-streamable-http-client-reqwest"
] }
```

**Step 2: Add CLI flag**

In `src/cli.rs`, add to both `Serve` and `Run` variants:

```rust
/// Upstream MCP servers (name=command_or_url)
#[arg(long = "mcp", num_args = 1)]
mcp_servers: Vec<String>,
```

**Step 3: Add parse_mcp_arg**

In `src/config.rs`:

```rust
/// Parse `name=command_or_url` CLI arg into an `McpServerConfigEntry`.
///
/// If the value starts with `http://` or `https://`, it's treated as a URL
/// (transport defaults to streamable-http). Otherwise, it's split on spaces
/// where the first token is the command and the rest are args.
pub fn parse_mcp_arg(arg: &str) -> anyhow::Result<(String, McpServerConfigEntry)> {
    let eq_pos = arg.find('=').ok_or_else(|| {
        anyhow::anyhow!("invalid --mcp format '{arg}': expected name=command_or_url")
    })?;
    let name = &arg[..eq_pos];
    let value = &arg[eq_pos + 1..];
    if name.is_empty() || value.is_empty() {
        anyhow::bail!("invalid --mcp format '{arg}': name and value must be non-empty");
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        Ok((name.to_string(), McpServerConfigEntry {
            command: None,
            args: None,
            env: None,
            url: Some(value.to_string()),
            transport: None,
        }))
    } else {
        let parts: Vec<&str> = value.split_whitespace().collect();
        let command = parts[0].to_string();
        let args = if parts.len() > 1 {
            Some(parts[1..].iter().map(|s| (*s).to_string()).collect())
        } else {
            None
        };
        Ok((name.to_string(), McpServerConfigEntry {
            command: Some(command),
            args,
            env: None,
            url: None,
            transport: None,
        }))
    }
}
```

**Step 4: Write tests for parse_mcp_arg**

```rust
#[test]
fn test_parse_mcp_arg_command() {
    let (name, entry) = parse_mcp_arg("filesystem=npx -y @modelcontextprotocol/server-filesystem /tmp").unwrap();
    assert_eq!(name, "filesystem");
    assert_eq!(entry.command.as_deref(), Some("npx"));
    assert_eq!(entry.args.as_ref().unwrap().len(), 3);
}

#[test]
fn test_parse_mcp_arg_url() {
    let (name, entry) = parse_mcp_arg("remote=https://mcp.example.com/mcp").unwrap();
    assert_eq!(name, "remote");
    assert_eq!(entry.url.as_deref(), Some("https://mcp.example.com/mcp"));
    assert!(entry.command.is_none());
}

#[test]
fn test_parse_mcp_arg_invalid() {
    assert!(parse_mcp_arg("noequals").is_err());
    assert!(parse_mcp_arg("=value").is_err());
    assert!(parse_mcp_arg("name=").is_err());
}
```

**Step 5: Run tests**

Run: `cargo test`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add Cargo.toml src/cli.rs src/config.rs
git commit -m "feat: add --mcp CLI flag and rmcp client features"
```

---

### Task 9: JSON Schema to Luau Shared Utilities

**Files:**
- Create: `src/codegen/luau_types.rs`
- Modify: `src/codegen/mod.rs` (add module)
- Test: `src/codegen/luau_types.rs`

**Step 1: Write the failing tests**

Create `src/codegen/luau_types.rs` with tests first:

```rust
//! Shared JSON Schema to Luau type conversion utilities.
//!
//! Used by both the OpenAPI codegen path and MCP tool schema conversion.

use serde_json::Value;

use super::manifest::{FieldDef, FieldType, McpParamDef, SchemaDef};

// Implementation goes here after tests

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn test_json_schema_type_to_luau() {
        assert_eq!(json_schema_type_to_luau("string", None), "string");
        assert_eq!(json_schema_type_to_luau("integer", None), "number");
        assert_eq!(json_schema_type_to_luau("number", None), "number");
        assert_eq!(json_schema_type_to_luau("boolean", None), "boolean");
        assert_eq!(json_schema_type_to_luau("unknown", None), "any");
    }

    #[test]
    fn test_json_schema_to_params_flat() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string", "description": "File path" },
                "encoding": { "type": "string" }
            }
        });
        let params = json_schema_to_params(&schema);
        assert_eq!(params.len(), 2);
        let path_param = params.iter().find(|p| p.name == "path").unwrap();
        assert!(path_param.required);
        assert_eq!(path_param.luau_type, "string");
        assert_eq!(path_param.description.as_deref(), Some("File path"));
        let enc_param = params.iter().find(|p| p.name == "encoding").unwrap();
        assert!(!enc_param.required);
    }

    #[test]
    fn test_extract_schema_defs_from_json_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "user": { "$ref": "#/$defs/User" }
            },
            "$defs": {
                "User": {
                    "type": "object",
                    "required": ["name"],
                    "properties": {
                        "name": { "type": "string" },
                        "email": { "type": "string" }
                    }
                }
            }
        });
        let defs = extract_schema_defs(&schema);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "User");
        assert_eq!(defs[0].fields.len(), 2);
    }

    #[test]
    fn test_json_schema_to_field_type() {
        let prop = serde_json::json!({ "type": "string" });
        assert_eq!(json_schema_prop_to_field_type(&prop), FieldType::String);

        let arr = serde_json::json!({ "type": "array", "items": { "type": "integer" } });
        assert_eq!(
            json_schema_prop_to_field_type(&arr),
            FieldType::Array { items: Box::new(FieldType::Integer) }
        );

        let obj = serde_json::json!({
            "type": "object",
            "properties": {
                "x": { "type": "number" }
            }
        });
        match json_schema_prop_to_field_type(&obj) {
            FieldType::InlineObject { fields } => {
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].name, "x");
            }
            other => panic!("Expected InlineObject, got {other:?}"),
        }

        let reftype = serde_json::json!({ "$ref": "#/$defs/User" });
        assert_eq!(
            json_schema_prop_to_field_type(&reftype),
            FieldType::Object { schema: "User".to_string() }
        );
    }
}
```

**Step 2: Run tests to verify they fail**

Expected: FAIL — functions don't exist

**Step 3: Write minimal implementation**

```rust
/// Convert a JSON Schema type string to a Luau type string.
pub fn json_schema_type_to_luau(type_str: &str, items: Option<&Value>) -> String {
    match type_str {
        "string" => "string".to_string(),
        "integer" | "number" => "number".to_string(),
        "boolean" => "boolean".to_string(),
        "array" => {
            let inner = items
                .and_then(|i| i.get("type"))
                .and_then(Value::as_str)
                .map_or_else(|| "any".to_string(), |t| json_schema_type_to_luau(t, None));
            format!("{{{inner}}}")
        }
        _ => "any".to_string(),
    }
}

/// Convert a JSON Schema input_schema to a list of McpParamDefs.
pub fn json_schema_to_params(schema: &Value) -> Vec<McpParamDef> {
    let properties = match schema.get("properties").and_then(Value::as_object) {
        Some(p) => p,
        None => return vec![],
    };
    let required: std::collections::HashSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    properties
        .iter()
        .map(|(name, prop)| {
            let type_str = prop.get("type").and_then(Value::as_str).unwrap_or("any");
            let items = prop.get("items");
            McpParamDef {
                name: name.clone(),
                luau_type: json_schema_type_to_luau(type_str, items),
                required: required.contains(name.as_str()),
                description: prop.get("description").and_then(Value::as_str).map(String::from),
            }
        })
        .collect()
}

/// Convert a JSON Schema property to a FieldType.
pub fn json_schema_prop_to_field_type(prop: &Value) -> FieldType {
    // Handle $ref
    if let Some(ref_str) = prop.get("$ref").and_then(Value::as_str) {
        let name = ref_str
            .rsplit('/')
            .next()
            .unwrap_or(ref_str);
        return FieldType::Object { schema: name.to_string() };
    }

    let type_str = prop.get("type").and_then(Value::as_str).unwrap_or("any");
    match type_str {
        "string" => FieldType::String,
        "integer" => FieldType::Integer,
        "number" => FieldType::Number,
        "boolean" => FieldType::Boolean,
        "array" => {
            let items_type = prop
                .get("items")
                .map(json_schema_prop_to_field_type)
                .unwrap_or(FieldType::String);
            FieldType::Array { items: Box::new(items_type) }
        }
        "object" => {
            if let Some(props) = prop.get("properties").and_then(Value::as_object) {
                let required: std::collections::HashSet<&str> = prop
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let fields: Vec<FieldDef> = props
                    .iter()
                    .map(|(name, p)| {
                        let field_type = json_schema_prop_to_field_type(p);
                        FieldDef {
                            name: name.clone(),
                            field_type,
                            required: required.contains(name.as_str()),
                            description: p.get("description").and_then(Value::as_str).map(String::from),
                            enum_values: None,
                            nullable: false,
                            format: None,
                        }
                    })
                    .collect();
                FieldType::InlineObject { fields }
            } else {
                FieldType::Map { value: Box::new(FieldType::String) }
            }
        }
        _ => FieldType::String, // fallback
    }
}

/// Extract named schemas from $defs or definitions in a JSON Schema.
pub fn extract_schema_defs(schema: &Value) -> Vec<SchemaDef> {
    let defs = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .and_then(Value::as_object);

    let Some(defs) = defs else {
        return vec![];
    };

    defs.iter()
        .map(|(name, def)| {
            let description = def.get("description").and_then(Value::as_str).map(String::from);
            let required: std::collections::HashSet<&str> = def
                .get("required")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let fields = def
                .get("properties")
                .and_then(Value::as_object)
                .map(|props| {
                    props
                        .iter()
                        .map(|(fname, fprop)| {
                            let field_type = json_schema_prop_to_field_type(fprop);
                            FieldDef {
                                name: fname.clone(),
                                field_type,
                                required: required.contains(fname.as_str()),
                                description: fprop.get("description").and_then(Value::as_str).map(String::from),
                                enum_values: None,
                                nullable: false,
                                format: None,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            SchemaDef {
                name: name.clone(),
                description,
                fields,
            }
        })
        .collect()
}
```

Add `pub mod luau_types;` to `src/codegen/mod.rs`.

**Step 4: Run tests**

Run: `cargo test --lib codegen::luau_types::tests`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add src/codegen/luau_types.rs src/codegen/mod.rs
git commit -m "feat: shared JSON Schema to Luau type conversion utilities"
```

---

### Task 10: MCP Client Connection Manager

**Files:**
- Create: `src/runtime/mcp_client.rs`
- Modify: `src/runtime/mod.rs`
- Test: `src/runtime/mcp_client.rs`

This module manages connecting to upstream MCP servers, listing their tools, converting tool schemas, and holding live client handles. It also handles reconnection on transport failure.

The implementation is larger and depends on rmcp client APIs. The key struct:

```rust
pub struct McpClientManager {
    clients: HashMap<String, McpClientHandle>,
}

struct McpClientHandle {
    service: RunningService<RoleClient, ()>,
    config: McpServerResolvedConfig, // needed for reconnect
}
```

Key methods:
- `connect_all(configs) -> Result<Self>` — connect to all configured servers
- `list_tools(name) -> Result<Vec<Tool>>` — get tools from a specific server
- `call_tool(server, tool_name, arguments) -> Result<CallToolResult>` — call with auto-reconnect
- `close_all()` — graceful shutdown

**This task includes writing the full connection logic for stdio, SSE, and streamable HTTP transports.** Write unit tests using a mock in-process MCP server built with rmcp's server side.

**Step 1-6:** Follow TDD — write tests for connect, list_tools, call_tool, reconnect, then implement.

**Commit:**
```bash
git commit -m "feat: MCP client connection manager with auto-reconnect"
```

---

### Task 11: MCP Tool Annotation Rendering

**Files:**
- Modify: `src/codegen/annotations.rs`
- Test: `src/codegen/annotations.rs`

Add `render_mcp_tool_annotation` and `render_mcp_tool_docs` (docs = annotation + referenced schemas):

```rust
pub fn render_mcp_tool_annotation(tool: &McpToolDef) -> String { ... }
pub fn render_mcp_tool_docs(tool: &McpToolDef) -> String { ... }
```

The annotation format:

```luau
-- Read the complete contents of a file
-- @param path - File path to read
function sdk.filesystem.read_file(params: { path: string }): any end
```

`render_mcp_tool_docs` appends any schemas from `tool.schemas` and `tool.output_schemas`.

Follow TDD — tests first, implementation after.

**Commit:**
```bash
git commit -m "feat: MCP tool Luau annotation and docs rendering"
```

---

### Task 12: Register MCP Tools in Luau Sandbox

**Files:**
- Modify: `src/runtime/registry.rs` (add `register_mcp_tools`)
- Modify: `src/runtime/executor.rs` (call `register_mcp_tools`)
- Test: `src/runtime/registry.rs`

Add `register_mcp_tools` that creates `sdk.<server>.<tool>()` closures backed by `McpClientManager::call_tool`. Follow the same pattern as `register_functions` — create Lua closures that call `block_in_place` + `block_on`.

The executor's `execute` method needs access to the `McpClientManager` (pass as `Arc<McpClientManager>`).

Follow TDD — tests with mock MCP server first.

**Commit:**
```bash
git commit -m "feat: register MCP tools as Luau closures in sdk namespace"
```

---

### Task 13: Wire MCP Into Discovery Tools

**Files:**
- Modify: `src/server/tools.rs` (`list_functions_impl`, `search_docs_impl`)
- Modify: `src/server/mod.rs` (annotation cache includes MCP tools)
- Modify: `src/server/resources.rs` (add MCP server resources)
- Test: `src/server/mod.rs`

`list_functions_impl` — append MCP tools to the results. Filter by server name using the existing `api` parameter.

`search_docs_impl` — search MCP tool names, descriptions, and param names.

`annotation_cache` — include MCP tool docs keyed by `server.tool_name`.

Resources — add `sdk://<server>/overview` and `sdk://<server>/functions`.

**Commit:**
```bash
git commit -m "feat: MCP tools appear in list_functions, search_docs, get_function_docs"
```

---

### Task 14: Wire MCP Into main.rs

**Files:**
- Modify: `src/main.rs`
- Modify: `src/server/mod.rs` (`ToolScriptServer::new`)

Wire the full pipeline:
1. Parse `--mcp` CLI flags and/or `[mcp_servers]` from config
2. Validate entries
3. Connect via `McpClientManager::connect_all`
4. Call `list_all_tools` on each, convert to `McpToolDef`s, populate manifest
5. Pass `McpClientManager` to `ToolScriptServer::new` → `ScriptExecutor`
6. Update `resolve_run_inputs` to allow MCP-only mode (no specs required if mcp_servers exist)

Update `ServeArgs` to carry the MCP config. Update `serve()` to connect and pass through.

**Commit:**
```bash
git commit -m "feat: wire upstream MCP servers into run/serve pipeline"
```

---

### Task 15: Integration Tests

**Files:**
- Create: `tests/mcp_integration.rs`

Write integration tests:
- MCP-only mode: start with only MCP servers, verify `list_functions` returns MCP tools, execute script that calls MCP tool
- Mixed mode: OpenAPI + MCP, verify both appear in `sdk.*` namespace
- get_function_docs includes schemas from MCP tool `$defs`
- Reconnection: test that a failed call triggers reconnect

Use an in-process mock MCP server for all tests.

**Commit:**
```bash
git commit -m "test: integration tests for upstream MCP server support"
```

---

### Task 16: Update server_info and Instructions

**Files:**
- Modify: `src/server/mod.rs:72-120` (`server_info`)

Update the description and instructions to mention MCP servers alongside APIs. Include MCP server names in the API list. Update the instructions text to mention that `sdk.*` includes both OpenAPI and MCP tools.

**Commit:**
```bash
git commit -m "feat: server_info includes MCP servers in description and instructions"
```

---

Plan complete and saved to `docs/plans/2026-02-26-upstream-mcp-impl.md`. Two execution options:

**1. Subagent-Driven (this session)** - I dispatch fresh subagent per task, review between tasks, fast iteration

**2. Parallel Session (separate)** - Open new session with executing-plans, batch execution with checkpoints

Which approach?
