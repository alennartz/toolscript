# code-mcp Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Rust CLI that takes OpenAPI specs and produces an MCP server with a Lua scripting runtime and auto-generated SDK.

**Architecture:** Single Rust binary with three modes (generate, serve, run). Codegen reads OpenAPI specs and emits a JSON manifest + Lua annotation files. Runtime loads the manifest, registers sandboxed Lua functions backed by Rust HTTP calls, and serves them via MCP.

**Tech Stack:** Rust, mlua (Lua 5.4), rmcp (MCP server), openapiv3 (OpenAPI parsing), reqwest (HTTP), tokio (async), clap (CLI), serde (serialization).

**Design doc:** `docs/plans/2026-02-21-code-mcp-design.md`

---

## Project Structure

```
code-mcp/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── cli.rs
│   ├── codegen/
│   │   ├── mod.rs
│   │   ├── parser.rs
│   │   ├── manifest.rs
│   │   └── annotations.rs
│   ├── runtime/
│   │   ├── mod.rs
│   │   ├── sandbox.rs
│   │   ├── registry.rs
│   │   └── http.rs
│   └── server/
│       ├── mod.rs
│       ├── tools.rs
│       └── resources.rs
├── testdata/
│   └── petstore.yaml
└── Dockerfile
```

---

### Task 1: Project Init & CLI Skeleton

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/cli.rs`

**Step 1: Create Cargo.toml with all dependencies**

```toml
[package]
name = "code-mcp"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
openapiv3 = "2"
mlua = { version = "0.10", features = ["lua54", "vendored", "async", "send", "serialize"] }
rmcp = { version = "0.1", features = ["server", "transport-sse-server", "transport-io"] }
reqwest = { version = "0.12", features = ["json"] }
anyhow = "1"
thiserror = "2"
url = { version = "2", features = ["serde"] }
schemars = "0.8"
```

Note: Pin exact rmcp version after verifying latest on crates.io. The version above is a placeholder — check with `cargo search rmcp` before proceeding.

**Step 2: Create CLI with clap derive**

`src/cli.rs`:
```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "code-mcp", about = "Generate MCP servers from OpenAPI specs")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Generate manifest and SDK annotations from OpenAPI specs
    Generate {
        /// OpenAPI spec sources (file paths or URLs)
        #[arg(required = true)]
        specs: Vec<String>,
        /// Output directory
        #[arg(short, long, default_value = "./output")]
        output: PathBuf,
    },
    /// Start MCP server from a generated directory
    Serve {
        /// Path to generated output directory
        #[arg(required = true)]
        dir: PathBuf,
        /// Transport type
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Port for SSE transport
        #[arg(long, default_value = "8080")]
        port: u16,
    },
    /// Generate and serve in one step
    Run {
        /// OpenAPI spec sources (file paths or URLs)
        #[arg(required = true)]
        specs: Vec<String>,
        /// Transport type
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Port for SSE transport
        #[arg(long, default_value = "8080")]
        port: u16,
    },
}
```

`src/main.rs`:
```rust
mod cli;

use clap::Parser;
use cli::{Cli, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Generate { specs, output } => {
            println!("Generate: {:?} -> {:?}", specs, output);
            todo!("generate command")
        }
        Command::Serve { dir, transport, port } => {
            println!("Serve: {:?} ({} on {})", dir, transport, port);
            todo!("serve command")
        }
        Command::Run { specs, transport, port } => {
            println!("Run: {:?} ({} on {})", specs, transport, port);
            todo!("run command")
        }
    }
}
```

**Step 3: Verify it compiles and runs**

```bash
cd /home/alenna/repos/code-mcp && cargo build
cargo run -- --help
cargo run -- generate --help
cargo run -- serve --help
cargo run -- run --help
```

Expected: Help text showing all three subcommands with their args.

**Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/
git commit -m "feat: project init with CLI skeleton (generate, serve, run)"
```

---

### Task 2: Test Data & OpenAPI Parsing

**Files:**
- Create: `testdata/petstore.yaml` (minimal but complete petstore spec)
- Create: `src/codegen/mod.rs`
- Create: `src/codegen/parser.rs`

**Step 1: Create test data**

`testdata/petstore.yaml` — a minimal OpenAPI 3.0 spec with enough variety to test all v1 features:
- 2-3 endpoints (GET with path param, GET with query params, POST with body)
- 2-3 schemas with various field types
- Tags for grouping
- Descriptions and examples on everything
- Bearer auth security scheme

Use the Swagger Petstore as a starting point but strip it down to essentials. Must include:
- `info` with title, description, version
- `servers` with a base URL
- `paths` with at least: `GET /pets` (query params: status, limit), `GET /pets/{petId}` (path param), `POST /pets` (request body)
- `components/schemas` with `Pet` (id, name, status, tag) and `NewPet` (name, tag)
- `components/securitySchemes` with bearer auth
- `tags` with descriptions
- `externalDocs` with URL
- Descriptions, summaries, and examples on operations, parameters, and schema fields

**Step 2: Write failing test for spec loading**

In `src/codegen/parser.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_load_spec_from_file() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert_eq!(spec.info.title, "Petstore");
        assert!(!spec.paths.paths.is_empty());
    }
}
```

**Step 3: Run test, verify it fails**

```bash
cargo test codegen::parser::tests::test_load_spec_from_file
```

Expected: FAIL — `load_spec_from_file` doesn't exist.

**Step 4: Implement spec loading**

`src/codegen/parser.rs`:
```rust
use anyhow::{Context, Result};
use openapiv3::OpenAPI;
use std::path::Path;

/// Load an OpenAPI spec from a local YAML or JSON file.
pub fn load_spec_from_file(path: &Path) -> Result<OpenAPI> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read spec file: {}", path.display()))?;
    parse_spec(&content)
}

/// Load an OpenAPI spec from a URL.
pub async fn load_spec_from_url(url: &str) -> Result<OpenAPI> {
    let content = reqwest::get(url).await?.text().await?;
    parse_spec(&content)
}

/// Parse an OpenAPI spec from a string (YAML or JSON).
fn parse_spec(content: &str) -> Result<OpenAPI> {
    // Try YAML first (it's a superset of JSON)
    serde_yaml::from_str(content).context("Failed to parse OpenAPI spec")
}
```

`src/codegen/mod.rs`:
```rust
pub mod parser;
```

Update `src/main.rs` to add `mod codegen;`.

**Step 5: Run test, verify it passes**

```bash
cargo test codegen::parser::tests::test_load_spec_from_file
```

Expected: PASS

**Step 6: Commit**

```bash
git add testdata/ src/codegen/ src/main.rs
git commit -m "feat: OpenAPI spec loading from file and URL"
```

---

### Task 3: Manifest Data Model

**Files:**
- Create: `src/codegen/manifest.rs`

The manifest is the central data structure that connects codegen to runtime.

**Step 1: Write failing test for manifest serialization roundtrip**

In `src/codegen/manifest.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_roundtrip() {
        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "petstore".into(),
                base_url: "https://petstore.example.com/v1".into(),
                description: Some("A sample API".into()),
                version: Some("1.0.0".into()),
                auth: Some(AuthConfig::Bearer {
                    header: "Authorization".into(),
                    prefix: "Bearer ".into(),
                }),
            }],
            functions: vec![FunctionDef {
                name: "get_pet".into(),
                api: "petstore".into(),
                tag: Some("pets".into()),
                method: HttpMethod::Get,
                path: "/pets/{pet_id}".into(),
                summary: Some("Get a pet by ID".into()),
                description: Some("Returns a single pet".into()),
                deprecated: false,
                parameters: vec![ParamDef {
                    name: "pet_id".into(),
                    location: ParamLocation::Path,
                    param_type: ParamType::String,
                    required: true,
                    description: Some("The pet's ID".into()),
                    default: None,
                    enum_values: None,
                }],
                request_body: None,
                response_schema: Some("Pet".into()),
            }],
            schemas: vec![SchemaDef {
                name: "Pet".into(),
                description: Some("A pet in the store".into()),
                fields: vec![
                    FieldDef {
                        name: "id".into(),
                        field_type: FieldType::String,
                        required: true,
                        description: Some("The pet's ID".into()),
                        enum_values: None,
                    },
                    FieldDef {
                        name: "name".into(),
                        field_type: FieldType::String,
                        required: true,
                        description: Some("The pet's name".into()),
                        enum_values: None,
                    },
                    FieldDef {
                        name: "status".into(),
                        field_type: FieldType::String,
                        required: false,
                        description: Some("Adoption status".into()),
                        enum_values: Some(vec![
                            "available".into(),
                            "pending".into(),
                            "sold".into(),
                        ]),
                    },
                ],
            }],
        };

        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let parsed: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.apis[0].name, "petstore");
        assert_eq!(parsed.functions[0].name, "get_pet");
        assert_eq!(parsed.functions[0].parameters.len(), 1);
        assert_eq!(parsed.schemas[0].fields.len(), 3);
    }
}
```

**Step 2: Run test, verify it fails**

```bash
cargo test codegen::manifest::tests::test_manifest_roundtrip
```

**Step 3: Implement manifest data structures**

`src/codegen/manifest.rs` — define all the types with Serialize/Deserialize:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub apis: Vec<ApiConfig>,
    pub functions: Vec<FunctionDef>,
    pub schemas: Vec<SchemaDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub name: String,
    pub base_url: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub auth: Option<AuthConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    Bearer { header: String, prefix: String },
    ApiKey { header: String },
    Basic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub api: String,
    pub tag: Option<String>,
    pub method: HttpMethod,
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub parameters: Vec<ParamDef>,
    pub request_body: Option<RequestBodyDef>,
    pub response_schema: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamDef {
    pub name: String,
    pub location: ParamLocation,
    pub param_type: ParamType,
    pub required: bool,
    pub description: Option<String>,
    pub default: Option<serde_json::Value>,
    pub enum_values: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamLocation {
    Path,
    Query,
    Header,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestBodyDef {
    pub content_type: String,
    pub schema: String,
    pub required: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaDef {
    pub name: String,
    pub description: Option<String>,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    pub field_type: FieldType,
    pub required: bool,
    pub description: Option<String>,
    pub enum_values: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    String,
    Integer,
    Number,
    Boolean,
    Array { items: Box<FieldType> },
    Object { schema: String },
}
```

Add `pub mod manifest;` to `src/codegen/mod.rs`.

**Step 4: Run test, verify it passes**

```bash
cargo test codegen::manifest::tests::test_manifest_roundtrip
```

**Step 5: Commit**

```bash
git add src/codegen/manifest.rs src/codegen/mod.rs
git commit -m "feat: manifest data model with serde serialization"
```

---

### Task 4: OpenAPI-to-Manifest Conversion

**Files:**
- Modify: `src/codegen/parser.rs` — add conversion functions

This is the core codegen logic: walking the OpenAPI spec and producing a Manifest.

**Step 1: Write failing test**

In `src/codegen/parser.rs`, add to the test module:
```rust
#[test]
fn test_spec_to_manifest() {
    let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
    let manifest = spec_to_manifest(&spec, "petstore").unwrap();

    // API config
    assert_eq!(manifest.apis.len(), 1);
    assert_eq!(manifest.apis[0].name, "petstore");
    assert!(!manifest.apis[0].base_url.is_empty());

    // Functions extracted from paths
    assert!(!manifest.functions.is_empty());
    // Should have at least: get /pets, get /pets/{petId}, post /pets
    let fn_names: Vec<&str> = manifest.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(fn_names.iter().any(|n| n.contains("pet")));

    // Schemas extracted from components
    assert!(!manifest.schemas.is_empty());
    let schema_names: Vec<&str> = manifest.schemas.iter().map(|s| s.name.as_str()).collect();
    assert!(schema_names.contains(&"Pet"));

    // Functions should have parameters
    let get_pet = manifest.functions.iter().find(|f| f.path.contains("{")).unwrap();
    assert!(!get_pet.parameters.is_empty());
    assert!(get_pet.parameters.iter().any(|p| p.location == ParamLocation::Path));

    // Functions should have descriptions from the spec
    for func in &manifest.functions {
        assert!(func.summary.is_some() || func.description.is_some(),
            "Function {} missing docs", func.name);
    }
}
```

**Step 2: Run test, verify it fails**

```bash
cargo test codegen::parser::tests::test_spec_to_manifest
```

**Step 3: Implement spec_to_manifest**

Add to `src/codegen/parser.rs`:

```rust
use crate::codegen::manifest::*;

/// Convert a parsed OpenAPI spec into a Manifest.
pub fn spec_to_manifest(spec: &OpenAPI, api_name: &str) -> Result<Manifest> {
    let api_config = extract_api_config(spec, api_name)?;
    let functions = extract_functions(spec, api_name)?;
    let schemas = extract_schemas(spec)?;

    Ok(Manifest {
        apis: vec![api_config],
        functions,
        schemas,
    })
}
```

Implement these helper functions:

- `extract_api_config` — reads `info`, `servers[0]`, and `securityDefinitions` to produce `ApiConfig`
- `extract_functions` — iterates `spec.paths`, for each path+method produces a `FunctionDef`. Derives function name from `operationId` (snake_case) or from method+path. Extracts parameters, request body ref, response schema ref, summary, description, tags, deprecated flag.
- `extract_schemas` — iterates `spec.components.schemas`, resolves each to a `SchemaDef` with `FieldDef` entries. Handles basic types, arrays, nested object refs, enums. Resolves `$ref` pointers within the spec.

Key details for the implementation:
- Use `openapiv3::ReferenceOr` — resolve refs via a helper that looks up `#/components/schemas/Name`
- For `operationId` naming: convert to snake_case (e.g., `listPets` → `list_pets`)
- For fallback naming: `{method}_{path_segments}` (e.g., `GET /pets/{petId}` → `get_pets_by_pet_id`)
- For parameter type mapping: `"string"` → `ParamType::String`, `"integer"` → `ParamType::Integer`, etc.
- For field type mapping: handle `"array"` with `items`, `"object"` with `$ref`, basic scalar types
- For auth: look at `securitySchemes` in components, map to `AuthConfig` variants

**Step 4: Run test, verify it passes**

```bash
cargo test codegen::parser::tests::test_spec_to_manifest
```

**Step 5: Add edge case tests**

Test for:
- Operations without `operationId` (fallback naming)
- Optional vs required parameters
- Operations with request bodies
- Auth scheme extraction

**Step 6: Run all tests**

```bash
cargo test
```

**Step 7: Commit**

```bash
git add src/codegen/parser.rs testdata/
git commit -m "feat: OpenAPI spec to manifest conversion"
```

---

### Task 5: Lua Annotation Generation

**Files:**
- Create: `src/codegen/annotations.rs`

**Step 1: Write failing test**

In `src/codegen/annotations.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::manifest::*;

    fn sample_manifest() -> Manifest {
        // ... (reuse the manifest from Task 3's test, or build a small one)
    }

    #[test]
    fn test_generate_function_annotation() {
        let func = FunctionDef {
            name: "get_pet".into(),
            api: "petstore".into(),
            tag: Some("pets".into()),
            method: HttpMethod::Get,
            path: "/pets/{pet_id}".into(),
            summary: Some("Get a pet by ID".into()),
            description: Some("Returns a single pet by its unique identifier.".into()),
            deprecated: false,
            parameters: vec![ParamDef {
                name: "pet_id".into(),
                location: ParamLocation::Path,
                param_type: ParamType::String,
                required: true,
                description: Some("The pet's unique identifier".into()),
                default: None,
                enum_values: None,
            }],
            request_body: None,
            response_schema: Some("Pet".into()),
        };

        let output = render_function_annotation(&func);
        assert!(output.contains("--- Get a pet by ID"));
        assert!(output.contains("--- Returns a single pet"));
        assert!(output.contains("--- @param pet_id string The pet's unique identifier"));
        assert!(output.contains("--- @return Pet"));
        assert!(output.contains("function sdk.get_pet(pet_id) end"));
    }

    #[test]
    fn test_generate_schema_annotation() {
        let schema = SchemaDef {
            name: "Pet".into(),
            description: Some("A pet in the store".into()),
            fields: vec![
                FieldDef {
                    name: "id".into(),
                    field_type: FieldType::String,
                    required: true,
                    description: Some("Unique ID".into()),
                    enum_values: None,
                },
                FieldDef {
                    name: "status".into(),
                    field_type: FieldType::String,
                    required: false,
                    description: Some("Current status".into()),
                    enum_values: Some(vec!["available".into(), "pending".into(), "sold".into()]),
                },
            ],
        };

        let output = render_schema_annotation(&schema);
        assert!(output.contains("--- @class Pet"));
        assert!(output.contains("--- A pet in the store"));
        assert!(output.contains("--- @field id string Unique ID"));
        assert!(output.contains(r#"--- @field status? "available"|"pending"|"sold" Current status"#));
    }

    #[test]
    fn test_generate_annotation_files() {
        let manifest = sample_manifest();
        let files = generate_annotation_files(&manifest);
        assert!(!files.is_empty());
        // Should have a file per tag or path group
        for (filename, content) in &files {
            assert!(filename.ends_with(".lua"));
            assert!(!content.is_empty());
        }
    }
}
```

**Step 2: Run tests, verify they fail**

```bash
cargo test codegen::annotations
```

**Step 3: Implement annotation rendering**

`src/codegen/annotations.rs`:

```rust
use crate::codegen::manifest::*;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Render a LuaLS-compatible annotation for a function.
pub fn render_function_annotation(func: &FunctionDef) -> String {
    let mut out = String::new();

    // Summary line
    if let Some(summary) = &func.summary {
        writeln!(out, "--- {summary}").unwrap();
    }
    // Extended description
    if let Some(desc) = &func.description {
        writeln!(out, "---").unwrap();
        for line in desc.lines() {
            writeln!(out, "--- {line}").unwrap();
        }
    }
    // Deprecated
    if func.deprecated {
        writeln!(out, "--- @deprecated").unwrap();
    }
    // Parameters
    for param in &func.parameters {
        let type_str = param_type_to_lua(&param.param_type, &param.enum_values);
        let opt = if param.required { "" } else { "?" };
        let desc = param.description.as_deref().unwrap_or("");
        writeln!(out, "--- @param {}{opt} {type_str} {desc}", param.name).unwrap();
    }
    // Request body parameter (if any)
    if let Some(body) = &func.request_body {
        let opt = if body.required { "" } else { "?" };
        let desc = body.description.as_deref().unwrap_or("Request body");
        writeln!(out, "--- @param body{opt} {} {desc}", body.schema).unwrap();
    }
    // Return type
    if let Some(response) = &func.response_schema {
        writeln!(out, "--- @return {response}").unwrap();
    }
    // Function signature
    let params: Vec<&str> = func.parameters.iter()
        .map(|p| p.name.as_str())
        .chain(func.request_body.as_ref().map(|_| "body"))
        .collect();
    writeln!(out, "function sdk.{}({}) end", func.name, params.join(", ")).unwrap();

    out
}

/// Render a LuaLS-compatible annotation for a schema (class).
pub fn render_schema_annotation(schema: &SchemaDef) -> String {
    // ... render @class and @field annotations
    // Handle optional fields (not required) with ? suffix on name
    // Handle enum fields with literal union type
    todo!()
}

/// Generate all annotation files grouped by tag.
pub fn generate_annotation_files(manifest: &Manifest) -> Vec<(String, String)> {
    // Group functions by tag (or "default" if no tag)
    // For each group, render header + all function annotations + referenced schemas
    todo!()
}

fn param_type_to_lua(pt: &ParamType, enum_values: &Option<Vec<String>>) -> String {
    if let Some(values) = enum_values {
        return values.iter().map(|v| format!("\"{v}\"")).collect::<Vec<_>>().join("|");
    }
    match pt {
        ParamType::String => "string".into(),
        ParamType::Integer => "integer".into(),
        ParamType::Number => "number".into(),
        ParamType::Boolean => "boolean".into(),
    }
}

fn field_type_to_lua(ft: &FieldType, enum_values: &Option<Vec<String>>) -> String {
    if let Some(values) = enum_values {
        return values.iter().map(|v| format!("\"{v}\"")).collect::<Vec<_>>().join("|");
    }
    match ft {
        FieldType::String => "string".into(),
        FieldType::Integer => "integer".into(),
        FieldType::Number => "number".into(),
        FieldType::Boolean => "boolean".into(),
        FieldType::Array { items } => format!("{}[]", field_type_to_lua(items, &None)),
        FieldType::Object { schema } => schema.clone(),
    }
}
```

Implement `render_schema_annotation` and `generate_annotation_files` following the patterns in the design doc. The file header should include API name, version, description from `manifest.apis`.

Add `pub mod annotations;` to `src/codegen/mod.rs`.

**Step 4: Run tests, verify they pass**

```bash
cargo test codegen::annotations
```

**Step 5: Commit**

```bash
git add src/codegen/annotations.rs src/codegen/mod.rs
git commit -m "feat: Lua annotation generation from manifest"
```

---

### Task 6: Generate Subcommand (End-to-End Codegen)

**Files:**
- Modify: `src/main.rs` — wire up generate command
- Create: `src/codegen/generate.rs` — orchestration function

**Step 1: Write failing integration test**

Create `tests/codegen_integration.rs`:
```rust
use std::path::Path;

#[test]
fn test_generate_from_petstore() {
    let output_dir = tempfile::tempdir().unwrap();
    code_mcp::codegen::generate(
        &["testdata/petstore.yaml".to_string()],
        output_dir.path(),
    ).unwrap();

    // manifest.json exists and is valid
    let manifest_path = output_dir.path().join("manifest.json");
    assert!(manifest_path.exists());
    let manifest: code_mcp::codegen::manifest::Manifest =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert!(!manifest.functions.is_empty());

    // sdk/ directory has .lua files
    let sdk_dir = output_dir.path().join("sdk");
    assert!(sdk_dir.exists());
    let lua_files: Vec<_> = std::fs::read_dir(&sdk_dir).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "lua").unwrap_or(false))
        .collect();
    assert!(!lua_files.is_empty());
}
```

Add `tempfile = "3"` to `[dev-dependencies]` in `Cargo.toml`.

**Step 2: Run test, verify it fails**

```bash
cargo test --test codegen_integration
```

**Step 3: Implement the generate orchestration**

`src/codegen/generate.rs`:
```rust
use crate::codegen::{annotations, manifest::Manifest, parser};
use anyhow::{Context, Result};
use std::path::Path;

/// Run the full codegen pipeline: parse specs, generate manifest and annotations.
pub fn generate(specs: &[String], output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;
    let sdk_dir = output_dir.join("sdk");
    std::fs::create_dir_all(&sdk_dir)?;

    let mut combined = Manifest {
        apis: vec![],
        functions: vec![],
        schemas: vec![],
    };

    for spec_source in specs {
        let spec = parser::load_spec_from_file(Path::new(spec_source))
            .with_context(|| format!("Failed to load spec: {spec_source}"))?;
        let api_name = derive_api_name(&spec, spec_source);
        let manifest = parser::spec_to_manifest(&spec, &api_name)?;
        combined.apis.extend(manifest.apis);
        combined.functions.extend(manifest.functions);
        combined.schemas.extend(manifest.schemas);
    }

    // Write manifest.json
    let manifest_json = serde_json::to_string_pretty(&combined)?;
    std::fs::write(output_dir.join("manifest.json"), manifest_json)?;

    // Write annotation files
    let files = annotations::generate_annotation_files(&combined);
    for (filename, content) in files {
        std::fs::write(sdk_dir.join(filename), content)?;
    }

    Ok(())
}

fn derive_api_name(spec: &openapiv3::OpenAPI, source: &str) -> String {
    // Use spec title (lowercased, underscored) or filename stem
    spec.info.title.to_lowercase().replace(' ', "_")
}
```

Make `generate` and all necessary types public in `src/lib.rs` (create this file to expose the library interface for integration tests):

```rust
// src/lib.rs
pub mod codegen;
```

Wire up in `src/main.rs`:
```rust
Command::Generate { specs, output } => {
    codegen::generate::generate(&specs, &output)?;
    println!("Generated output to {}", output.display());
    Ok(())
}
```

**Step 4: Run integration test**

```bash
cargo test --test codegen_integration
```

**Step 5: Also test via CLI**

```bash
cargo run -- generate testdata/petstore.yaml -o /tmp/code-mcp-test
cat /tmp/code-mcp-test/manifest.json
cat /tmp/code-mcp-test/sdk/*.lua
```

Verify the output looks correct — manifest has functions and schemas, Lua files have proper annotations.

**Step 6: Commit**

```bash
git add src/lib.rs src/codegen/generate.rs src/main.rs tests/ Cargo.toml
git commit -m "feat: generate subcommand with end-to-end codegen pipeline"
```

---

### Task 7: Lua Sandbox

**Files:**
- Create: `src/runtime/mod.rs`
- Create: `src/runtime/sandbox.rs`

**Step 1: Write failing test**

In `src/runtime/sandbox.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_allows_basic_lua() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let result: String = sandbox.eval("return 'hello'").unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_sandbox_allows_string_lib() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let result: String = sandbox.eval("return string.upper('hello')").unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn test_sandbox_allows_table_lib() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let result: String = sandbox.eval(r#"
            local t = {3, 1, 2}
            table.sort(t)
            return table.concat(t, ",")
        "#).unwrap();
        assert_eq!(result, "1,2,3");
    }

    #[test]
    fn test_sandbox_allows_math_lib() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let result: f64 = sandbox.eval("return math.floor(3.7)").unwrap();
        assert_eq!(result, 3.0);
    }

    #[test]
    fn test_sandbox_blocks_io() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let result = sandbox.eval::<mlua::Value>("return io.open('/etc/passwd')");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_blocks_os_execute() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let result = sandbox.eval::<mlua::Value>("return os.execute('ls')");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_blocks_loadfile() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let result = sandbox.eval::<mlua::Value>("return loadfile('/etc/passwd')");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_blocks_require() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let result = sandbox.eval::<mlua::Value>("return require('os')");
        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_captures_print() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let (result, logs) = sandbox.eval_with_logs::<mlua::Value>(r#"
            print("hello")
            print("world")
            return nil
        "#).unwrap();
        assert_eq!(logs, vec!["hello", "world"]);
    }

    #[test]
    fn test_sandbox_has_json_helpers() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let result: String = sandbox.eval(r#"
            local t = {name = "test", value = 42}
            local encoded = json.encode(t)
            local decoded = json.decode(encoded)
            return decoded.name
        "#).unwrap();
        assert_eq!(result, "test");
    }
}
```

**Step 2: Run tests, verify they fail**

```bash
cargo test runtime::sandbox
```

**Step 3: Implement sandbox**

`src/runtime/sandbox.rs`:
```rust
use anyhow::Result;
use mlua::{Lua, Value, FromLua, Function, StdLib};
use std::sync::{Arc, Mutex};

pub struct SandboxConfig {
    pub memory_limit: Option<usize>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            memory_limit: Some(64 * 1024 * 1024), // 64MB
        }
    }
}

pub struct Sandbox {
    lua: Lua,
    logs: Arc<Mutex<Vec<String>>>,
}

impl Sandbox {
    pub fn new(config: SandboxConfig) -> Result<Self> {
        // Create Lua with only safe standard libraries
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH,
            mlua::LuaOptions::default(),
        )?;

        if let Some(limit) = config.memory_limit {
            lua.set_memory_limit(limit)?;
        }

        let logs = Arc::new(Mutex::new(Vec::new()));

        // Remove dangerous globals that might have leaked
        {
            let globals = lua.globals();
            for name in &["io", "os", "loadfile", "dofile", "require", "debug", "load"] {
                globals.set(*name, Value::Nil)?;
            }
        }

        // Add captured print
        let logs_clone = logs.clone();
        let print_fn = lua.create_function(move |_, args: mlua::MultiValue| {
            let parts: Vec<String> = args.iter().map(|v| format!("{:?}", v)).collect();
            // Better: use tostring-like formatting
            let line = parts.join("\t");
            logs_clone.lock().unwrap().push(line);
            Ok(())
        })?;
        lua.globals().set("print", print_fn)?;

        // Add json.encode / json.decode
        let json_table = lua.create_table()?;
        json_table.set("encode", lua.create_function(|lua, value: Value| {
            let json_value = lua.from_value::<serde_json::Value>(value)?;
            serde_json::to_string(&json_value)
                .map_err(|e| mlua::Error::external(e))
        })?)?;
        json_table.set("decode", lua.create_function(|lua, s: String| {
            let json_value: serde_json::Value = serde_json::from_str(&s)
                .map_err(|e| mlua::Error::external(e))?;
            lua.to_value(&json_value)
        })?)?;
        lua.globals().set("json", json_table)?;

        // Add sdk table (empty, will be populated by registry)
        lua.globals().set("sdk", lua.create_table()?)?;

        Ok(Self { lua, logs })
    }

    pub fn eval<T: FromLua>(&self, script: &str) -> Result<T> {
        let result = self.lua.load(script).eval::<T>()?;
        Ok(result)
    }

    pub fn eval_with_logs<T: FromLua>(&self, script: &str) -> Result<(T, Vec<String>)> {
        self.logs.lock().unwrap().clear();
        let result = self.lua.load(script).eval::<T>()?;
        let logs = self.logs.lock().unwrap().clone();
        Ok((result, logs))
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    pub fn take_logs(&self) -> Vec<String> {
        std::mem::take(&mut self.logs.lock().unwrap())
    }
}
```

`src/runtime/mod.rs`:
```rust
pub mod sandbox;
```

Add `mod runtime;` to `src/main.rs` and `pub mod runtime;` to `src/lib.rs`.

**Step 4: Run tests, verify they pass**

```bash
cargo test runtime::sandbox
```

Iterate on the sandbox implementation until all tests pass. Pay special attention to the `print` function formatting and the JSON helpers — mlua's serde integration handles the Lua↔JSON conversion.

**Step 5: Commit**

```bash
git add src/runtime/ src/main.rs src/lib.rs
git commit -m "feat: sandboxed Lua runtime with captured print and JSON helpers"
```

---

### Task 8: SDK Function Registration

**Files:**
- Create: `src/runtime/registry.rs`

This registers Rust-backed functions into the Lua `sdk` table based on the manifest.

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::manifest::*;
    use crate::runtime::sandbox::{Sandbox, SandboxConfig};

    fn mock_http_handler() -> HttpHandler {
        // A mock that returns canned JSON responses instead of making real HTTP calls
        HttpHandler::mock(|method, url, _headers, _body| {
            match (method.as_str(), url.as_str()) {
                ("GET", url) if url.contains("/pets/") => {
                    Ok(serde_json::json!({"id": "123", "name": "Fido", "status": "available"}))
                }
                _ => Err(anyhow::anyhow!("unexpected request: {} {}", method, url)),
            }
        })
    }

    #[test]
    fn test_register_and_call_function() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "petstore".into(),
                base_url: "https://petstore.example.com/v1".into(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![FunctionDef {
                name: "get_pet".into(),
                api: "petstore".into(),
                tag: None,
                method: HttpMethod::Get,
                path: "/pets/{pet_id}".into(),
                summary: None,
                description: None,
                deprecated: false,
                parameters: vec![ParamDef {
                    name: "pet_id".into(),
                    location: ParamLocation::Path,
                    param_type: ParamType::String,
                    required: true,
                    description: None,
                    default: None,
                    enum_values: None,
                }],
                request_body: None,
                response_schema: None,
            }],
            schemas: vec![],
        };

        let handler = mock_http_handler();
        register_functions(&sandbox, &manifest, handler).unwrap();

        let result: String = sandbox.eval(r#"
            local pet = sdk.get_pet("123")
            return pet.name
        "#).unwrap();
        assert_eq!(result, "Fido");
    }

    #[test]
    fn test_missing_required_param_errors() {
        let sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        // ... register a function with a required param
        // ... call it without the param
        // ... assert error message mentions the missing param
    }
}
```

**Step 2: Run tests, verify they fail**

```bash
cargo test runtime::registry
```

**Step 3: Implement function registration**

`src/runtime/registry.rs`:

The core idea: for each `FunctionDef` in the manifest, create a Rust closure that:
1. Validates the Lua arguments match the parameter definitions
2. Builds the HTTP request (substitute path params, add query params, serialize body)
3. Calls the `HttpHandler` (which makes the real HTTP call or returns a mock)
4. Parses the JSON response and converts it to a Lua table
5. Returns the Lua value

Register this closure as `sdk.<function_name>` in the Lua environment.

```rust
use crate::codegen::manifest::*;
use crate::runtime::sandbox::Sandbox;
use crate::runtime::http::HttpHandler;
use anyhow::Result;

pub fn register_functions(sandbox: &Sandbox, manifest: &Manifest, handler: HttpHandler) -> Result<()> {
    let lua = sandbox.lua();
    let sdk_table = lua.globals().get::<mlua::Table>("sdk")?;

    for func_def in &manifest.functions {
        let api = manifest.apis.iter()
            .find(|a| a.name == func_def.api)
            .ok_or_else(|| anyhow::anyhow!("API '{}' not found", func_def.api))?;

        let base_url = api.base_url.clone();
        let func_def = func_def.clone();
        let handler = handler.clone();
        let auth = api.auth.clone();

        let lua_fn = lua.create_function(move |lua, args: mlua::MultiValue| {
            // 1. Validate and extract args based on func_def.parameters
            // 2. Build URL with path param substitution
            // 3. Build query params
            // 4. Build request body if present
            // 5. Call handler.request(method, url, headers, body)
            // 6. Convert JSON response to Lua value
            todo!()
        })?;

        sdk_table.set(func_def.name.as_str(), lua_fn)?;
    }

    Ok(())
}
```

The implementation needs careful handling of:
- Positional Lua args mapped to named parameters (in the order they appear in the manifest)
- Path parameter substitution in URL templates
- Query parameter encoding
- Request body serialization
- Error conversion from Rust to Lua errors with useful messages

**Step 4: Run tests, verify they pass**

```bash
cargo test runtime::registry
```

**Step 5: Commit**

```bash
git add src/runtime/registry.rs src/runtime/mod.rs
git commit -m "feat: SDK function registration from manifest into Lua sandbox"
```

---

### Task 9: HTTP Client with Auth Injection

**Files:**
- Create: `src/runtime/http.rs`

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::manifest::AuthConfig;

    #[tokio::test]
    async fn test_bearer_auth_injection() {
        // Use a mock server (e.g., wiremock) or inspect request headers
        let handler = HttpHandler::new();
        let auth = AuthConfig::Bearer {
            header: "Authorization".into(),
            prefix: "Bearer ".into(),
        };
        let credentials = AuthCredentials::BearerToken("sk-test123".into());

        let request = handler.build_request(
            "GET",
            "https://example.com/api/test",
            &auth,
            &credentials,
            &[],
            None,
        );

        assert_eq!(
            request.headers().get("Authorization").unwrap().to_str().unwrap(),
            "Bearer sk-test123"
        );
    }

    #[tokio::test]
    async fn test_api_key_auth_injection() {
        let handler = HttpHandler::new();
        let auth = AuthConfig::ApiKey { header: "X-API-Key".into() };
        let credentials = AuthCredentials::ApiKey("my-key".into());

        let request = handler.build_request(
            "GET",
            "https://example.com/api/test",
            &auth,
            &credentials,
            &[],
            None,
        );

        assert_eq!(
            request.headers().get("X-API-Key").unwrap().to_str().unwrap(),
            "my-key"
        );
    }
}
```

**Step 2: Run tests, verify they fail**

```bash
cargo test runtime::http
```

**Step 3: Implement HTTP client**

`src/runtime/http.rs`:
```rust
use crate::codegen::manifest::AuthConfig;
use anyhow::Result;
use reqwest::Client;

#[derive(Clone)]
pub enum AuthCredentials {
    BearerToken(String),
    ApiKey(String),
    Basic { username: String, password: String },
    None,
}

#[derive(Clone)]
pub struct HttpHandler {
    client: Client,
    mock_fn: Option<Arc<dyn Fn(&str, &str, &[(String, String)], Option<&serde_json::Value>) -> Result<serde_json::Value> + Send + Sync>>,
}

impl HttpHandler {
    pub fn new() -> Self {
        Self { client: Client::new(), mock_fn: None }
    }

    pub fn mock<F>(f: F) -> Self
    where F: Fn(&str, &str, &[(String, String)], Option<&serde_json::Value>) -> Result<serde_json::Value> + Send + Sync + 'static {
        Self { client: Client::new(), mock_fn: Some(Arc::new(f)) }
    }

    pub async fn request(
        &self,
        method: &str,
        url: &str,
        auth: Option<&AuthConfig>,
        credentials: &AuthCredentials,
        query_params: &[(String, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value> {
        if let Some(mock) = &self.mock_fn {
            return mock(method, url, query_params, body);
        }

        let mut req = match method {
            "GET" => self.client.get(url),
            "POST" => self.client.post(url),
            "PUT" => self.client.put(url),
            "PATCH" => self.client.patch(url),
            "DELETE" => self.client.delete(url),
            _ => anyhow::bail!("unsupported method: {method}"),
        };

        // Inject auth
        if let (Some(auth_config), creds) = (auth, credentials) {
            match (auth_config, creds) {
                (AuthConfig::Bearer { header, prefix }, AuthCredentials::BearerToken(token)) => {
                    req = req.header(header.as_str(), format!("{prefix}{token}"));
                }
                (AuthConfig::ApiKey { header }, AuthCredentials::ApiKey(key)) => {
                    req = req.header(header.as_str(), key.as_str());
                }
                (AuthConfig::Basic, AuthCredentials::Basic { username, password }) => {
                    req = req.basic_auth(username, Some(password));
                }
                _ => {}
            }
        }

        // Query params
        if !query_params.is_empty() {
            req = req.query(query_params);
        }

        // Body
        if let Some(body) = body {
            req = req.json(body);
        }

        let response = req.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("API returned {status}: {body}");
        }
        let json = response.json().await?;
        Ok(json)
    }
}
```

Add `pub mod http;` to `src/runtime/mod.rs`.

**Step 4: Run tests, verify they pass**

```bash
cargo test runtime::http
```

**Step 5: Commit**

```bash
git add src/runtime/http.rs src/runtime/mod.rs
git commit -m "feat: HTTP client with auth injection for SDK function calls"
```

---

### Task 10: Script Executor

**Files:**
- Create: `src/runtime/executor.rs`

Ties sandbox + registry + HTTP together into a single execution interface.

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_script_returns_result() {
        let manifest = test_manifest(); // petstore manifest
        let handler = mock_handler();   // canned responses
        let executor = ScriptExecutor::new(manifest, handler, ExecutorConfig::default()).unwrap();

        let result = executor.execute(
            "local pet = sdk.get_pet('123')\nreturn pet.name",
            &AuthCredentialsMap::default(),
        ).await.unwrap();

        assert_eq!(result.result, serde_json::json!("Fido"));
        assert!(result.stats.api_calls > 0);
    }

    #[tokio::test]
    async fn test_execute_script_captures_logs() {
        let executor = ScriptExecutor::new(/* ... */).unwrap();
        let result = executor.execute(
            "print('hello')\nreturn 42",
            &AuthCredentialsMap::default(),
        ).await.unwrap();
        assert_eq!(result.logs, vec!["hello"]);
    }

    #[tokio::test]
    async fn test_execute_script_timeout() {
        let config = ExecutorConfig { timeout_ms: 100, ..Default::default() };
        let executor = ScriptExecutor::new(/* ... config ... */).unwrap();
        let result = executor.execute("while true do end", &AuthCredentialsMap::default()).await;
        assert!(result.is_err());
        // Error should indicate timeout
    }

    #[tokio::test]
    async fn test_execute_script_parse_error() {
        let executor = ScriptExecutor::new(/* ... */).unwrap();
        let result = executor.execute(
            "this is not valid lua!!!",
            &AuthCredentialsMap::default(),
        ).await;
        assert!(result.is_err());
    }
}
```

**Step 2: Run tests, verify they fail**

```bash
cargo test runtime::executor
```

**Step 3: Implement executor**

`src/runtime/executor.rs`:
```rust
use crate::codegen::manifest::Manifest;
use crate::runtime::http::{HttpHandler, AuthCredentials};
use crate::runtime::sandbox::{Sandbox, SandboxConfig};
use crate::runtime::registry;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type AuthCredentialsMap = HashMap<String, AuthCredentials>;

#[derive(Clone)]
pub struct ExecutorConfig {
    pub timeout_ms: u64,
    pub memory_limit: Option<usize>,
    pub max_api_calls: Option<usize>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            memory_limit: Some(64 * 1024 * 1024),
            max_api_calls: Some(100),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub result: serde_json::Value,
    pub logs: Vec<String>,
    pub stats: ExecutionStats,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutionStats {
    pub api_calls: usize,
    pub duration_ms: u64,
}

pub struct ScriptExecutor {
    manifest: Manifest,
    handler: HttpHandler,
    config: ExecutorConfig,
}

impl ScriptExecutor {
    pub fn new(manifest: Manifest, handler: HttpHandler, config: ExecutorConfig) -> Result<Self> {
        Ok(Self { manifest, handler, config })
    }

    pub async fn execute(
        &self,
        script: &str,
        auth: &AuthCredentialsMap,
    ) -> Result<ExecutionResult> {
        let start = std::time::Instant::now();

        // Create fresh sandbox per execution
        let sandbox = Sandbox::new(SandboxConfig {
            memory_limit: self.config.memory_limit,
        })?;

        // Register SDK functions with auth credentials
        registry::register_functions(&sandbox, &self.manifest, self.handler.clone())?;

        // Execute with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(self.config.timeout_ms),
            async {
                sandbox.eval::<mlua::Value>(script)
            }
        ).await??;

        // Serialize result to JSON
        let json_result = lua_value_to_json(&result)?;
        let logs = sandbox.take_logs();
        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(ExecutionResult {
            result: json_result,
            logs,
            stats: ExecutionStats {
                api_calls: 0, // TODO: track via handler
                duration_ms,
            },
        })
    }
}

fn lua_value_to_json(value: &mlua::Value) -> Result<serde_json::Value> {
    // Convert Lua values to serde_json::Value
    // Use mlua's serde integration
    todo!()
}
```

Add `pub mod executor;` to `src/runtime/mod.rs`.

**Step 4: Run tests, verify they pass**

```bash
cargo test runtime::executor
```

**Step 5: Commit**

```bash
git add src/runtime/executor.rs src/runtime/mod.rs
git commit -m "feat: script executor with timeout, logging, and stats"
```

---

### Task 11: MCP Server with Documentation Tools

**Files:**
- Create: `src/server/mod.rs`
- Create: `src/server/tools.rs`

Uses `rmcp` crate with `#[tool_router]` macro to define MCP tools.

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_apis() {
        let state = ServerState::new(test_manifest(), /* ... */);
        let result = state.list_apis();
        assert!(!result.is_empty());
        assert!(result[0].name == "petstore");
    }

    #[test]
    fn test_list_functions_all() {
        let state = ServerState::new(test_manifest(), /* ... */);
        let result = state.list_functions(None, None);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_list_functions_filtered_by_tag() {
        let state = ServerState::new(test_manifest(), /* ... */);
        let result = state.list_functions(None, Some("pets"));
        assert!(result.iter().all(|f| f.tag.as_deref() == Some("pets")));
    }

    #[test]
    fn test_get_function_docs() {
        let state = ServerState::new(test_manifest(), /* ... */);
        let result = state.get_function_docs("get_pet").unwrap();
        assert!(result.contains("@param"));
        assert!(result.contains("@return"));
    }

    #[test]
    fn test_search_docs() {
        let state = ServerState::new(test_manifest(), /* ... */);
        let results = state.search_docs("pet");
        assert!(!results.is_empty());
    }

    #[test]
    fn test_get_schema() {
        let state = ServerState::new(test_manifest(), /* ... */);
        let result = state.get_schema("Pet").unwrap();
        assert!(result.contains("@class Pet"));
        assert!(result.contains("@field"));
    }
}
```

**Step 2: Run tests, verify they fail**

```bash
cargo test server::tools
```

**Step 3: Implement server state and doc tools**

`src/server/tools.rs`:

Define a `ServerState` struct that holds the manifest and pre-rendered annotation strings. Implement the doc query methods. Then use `rmcp`'s `#[tool_router]` to expose them as MCP tools:

```rust
use crate::codegen::manifest::Manifest;
use crate::codegen::annotations;
use crate::runtime::executor::ScriptExecutor;
use rmcp::{tool, tool_router, handler::server::tool::ToolRouter, model::*};

#[derive(Clone)]
pub struct CodeMcpServer {
    manifest: Manifest,
    annotation_cache: HashMap<String, String>,  // function_name -> annotation text
    schema_cache: HashMap<String, String>,       // schema_name -> annotation text
    executor: Arc<ScriptExecutor>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl CodeMcpServer {
    pub fn new(manifest: Manifest, executor: ScriptExecutor) -> Self {
        let annotation_cache = build_annotation_cache(&manifest);
        let schema_cache = build_schema_cache(&manifest);
        Self {
            manifest,
            annotation_cache,
            schema_cache,
            executor: Arc::new(executor),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List all loaded APIs with their names, descriptions, and endpoint counts")]
    async fn list_apis(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        // Return JSON array of API summaries
        todo!()
    }

    #[tool(description = "List available SDK functions, optionally filtered by API or tag")]
    async fn list_functions(
        &self,
        #[tool(param, description = "Filter by API name")] api: Option<String>,
        #[tool(param, description = "Filter by tag")] tag: Option<String>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        todo!()
    }

    #[tool(description = "Get full documentation for a specific SDK function")]
    async fn get_function_docs(
        &self,
        #[tool(param, description = "Function name")] name: String,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        todo!()
    }

    #[tool(description = "Search across all SDK documentation")]
    async fn search_docs(
        &self,
        #[tool(param, description = "Search query")] query: String,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        todo!()
    }

    #[tool(description = "Get the full definition of a data type")]
    async fn get_schema(
        &self,
        #[tool(param, description = "Schema/type name")] name: String,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        todo!()
    }

    #[tool(description = "Execute a Lua script against the SDK")]
    async fn execute_script(
        &self,
        #[tool(param, description = "The Lua script to execute")] script: String,
        #[tool(param, description = "Execution timeout in milliseconds")] timeout_ms: Option<u64>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // Note: auth comes from server-side config or MCP client injection,
        // not from this parameter
        todo!()
    }
}
```

Note: The exact rmcp API may differ from what's shown — verify against the actual crate docs. The `#[tool(param, ...)]` attribute syntax is based on the rmcp examples; adapt as needed.

`src/server/mod.rs`:
```rust
pub mod tools;
```

Add `mod server;` to `src/main.rs` and `pub mod server;` to `src/lib.rs`.

**Step 4: Run tests, verify they pass**

```bash
cargo test server::tools
```

**Step 5: Commit**

```bash
git add src/server/ src/main.rs src/lib.rs
git commit -m "feat: MCP server with doc exploration and script execution tools"
```

---

### Task 12: MCP Resources

**Files:**
- Create: `src/server/resources.rs`

Expose SDK documentation as browsable MCP resources.

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_list() {
        let state = ServerState::new(test_manifest());
        let resources = state.list_resources();
        // Should include overview, functions, schemas, and per-item resources
        assert!(resources.iter().any(|r| r.uri.contains("overview")));
        assert!(resources.iter().any(|r| r.uri.contains("functions")));
        assert!(resources.iter().any(|r| r.uri.contains("schemas")));
    }

    #[test]
    fn test_read_overview_resource() {
        let state = ServerState::new(test_manifest());
        let content = state.read_resource("sdk://petstore/overview").unwrap();
        assert!(content.contains("petstore"));
    }

    #[test]
    fn test_read_function_resource() {
        let state = ServerState::new(test_manifest());
        let content = state.read_resource("sdk://petstore/functions/get_pet").unwrap();
        assert!(content.contains("get_pet"));
    }
}
```

**Step 2: Run tests, verify they fail**

```bash
cargo test server::resources
```

**Step 3: Implement resource serving**

Implement `ServerHandler` trait for `CodeMcpServer` to handle resource listing and reading. The resources serve the same annotation content as the doc tools, just via a different MCP interface.

Follow the rmcp resource serving API. The exact trait methods will depend on rmcp's version — check the crate docs.

Add `pub mod resources;` to `src/server/mod.rs`.

**Step 4: Run tests, verify they pass**

```bash
cargo test server::resources
```

**Step 5: Commit**

```bash
git add src/server/resources.rs src/server/mod.rs
git commit -m "feat: MCP resources for browsable SDK documentation"
```

---

### Task 13: Serve & Run Subcommands

**Files:**
- Modify: `src/main.rs` — wire up serve and run commands

**Step 1: Implement serve command**

Wire the MCP server startup into the serve subcommand:

```rust
Command::Serve { dir, transport, port } => {
    // 1. Load manifest from dir/manifest.json
    let manifest: Manifest = serde_json::from_str(
        &std::fs::read_to_string(dir.join("manifest.json"))?
    )?;

    // 2. Create executor
    let handler = HttpHandler::new();
    let executor = ScriptExecutor::new(manifest.clone(), handler, ExecutorConfig::default())?;

    // 3. Create MCP server
    let server = CodeMcpServer::new(manifest, executor);

    // 4. Start with selected transport
    match transport.as_str() {
        "stdio" => {
            let transport = rmcp::transport::io::stdio();
            server.serve(transport).await?;
        }
        "sse" => {
            // Start SSE server on the given port
            todo!("SSE transport setup")
        }
        _ => anyhow::bail!("Unknown transport: {transport}"),
    }
    Ok(())
}
```

**Step 2: Implement run command**

```rust
Command::Run { specs, transport, port } => {
    // 1. Generate to temp dir
    let temp_dir = tempfile::tempdir()?;
    codegen::generate::generate(&specs, temp_dir.path())?;

    // 2. Serve from temp dir (reuse serve logic)
    // ... same as serve but using temp_dir.path()
}
```

**Step 3: Test stdio transport manually**

```bash
# Generate first
cargo run -- generate testdata/petstore.yaml -o /tmp/code-mcp-test

# Serve (will read from stdin, write to stdout — use for MCP protocol testing)
echo '{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' | cargo run -- serve /tmp/code-mcp-test

# Or run directly
echo '{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' | cargo run -- run testdata/petstore.yaml
```

Expected: JSON-RPC response with server capabilities listing our tools and resources.

**Step 4: Test run command end-to-end**

```bash
cargo run -- run testdata/petstore.yaml
```

Should start the MCP server in stdio mode.

**Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: serve and run subcommands with stdio MCP transport"
```

---

### Task 14: SSE Transport

**Files:**
- Modify: `src/main.rs` — add SSE server startup

**Step 1: Implement SSE transport**

Using rmcp's SSE transport support (the `transport-sse-server` feature):

```rust
"sse" => {
    // Use rmcp's SSE server transport
    // Bind to 0.0.0.0:{port}
    // The exact API depends on rmcp version — check docs
    todo!()
}
```

Consult rmcp docs for the SSE server setup. It typically involves starting an HTTP server with an SSE endpoint.

**Step 2: Test SSE transport**

```bash
cargo run -- serve /tmp/code-mcp-test --transport sse --port 8080 &
# In another terminal, test the SSE endpoint
curl http://localhost:8080/sse
```

**Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: SSE transport for remote MCP server access"
```

---

### Task 15: Auth Configuration from Environment

**Files:**
- Modify: `src/runtime/executor.rs` or `src/server/tools.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_auth_from_env() {
    std::env::set_var("PETSTORE_BEARER_TOKEN", "sk-test");
    let auth = load_auth_from_env(&manifest);
    assert!(matches!(auth.get("petstore"), Some(AuthCredentials::BearerToken(t)) if t == "sk-test"));
    std::env::remove_var("PETSTORE_BEARER_TOKEN");
}
```

**Step 2: Implement env-based auth loading**

Convention: `{API_NAME_UPPER}_BEARER_TOKEN`, `{API_NAME_UPPER}_API_KEY`, or `{API_NAME_UPPER}_BASIC_USER` + `{API_NAME_UPPER}_BASIC_PASS`.

```rust
pub fn load_auth_from_env(manifest: &Manifest) -> AuthCredentialsMap {
    let mut map = HashMap::new();
    for api in &manifest.apis {
        let prefix = api.name.to_uppercase();
        if let Ok(token) = std::env::var(format!("{prefix}_BEARER_TOKEN")) {
            map.insert(api.name.clone(), AuthCredentials::BearerToken(token));
        } else if let Ok(key) = std::env::var(format!("{prefix}_API_KEY")) {
            map.insert(api.name.clone(), AuthCredentials::ApiKey(key));
        }
    }
    map
}
```

**Step 3: Run test, verify it passes**

```bash
cargo test test_auth_from_env
```

**Step 4: Commit**

```bash
git add src/
git commit -m "feat: load API credentials from environment variables"
```

---

### Task 16: Dockerfile

**Files:**
- Create: `Dockerfile`

**Step 1: Write Dockerfile**

```dockerfile
FROM rust:1.85-slim AS builder
WORKDIR /build
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/code-mcp /usr/local/bin/code-mcp
ENTRYPOINT ["code-mcp", "run"]
```

**Step 2: Build and test**

```bash
docker build -t code-mcp .
docker run code-mcp --help
```

Expected: Help text from the CLI.

**Step 3: Test with a spec**

```bash
# Test with a local spec
docker run -v $(pwd)/testdata:/specs code-mcp /specs/petstore.yaml --transport sse --port 8080
```

**Step 4: Commit**

```bash
git add Dockerfile
git commit -m "feat: Dockerfile for containerized deployment"
```

---

### Task 17: Integration Test — Full Round-Trip

**Files:**
- Create: `tests/full_roundtrip.rs`

This is the final validation: start from an OpenAPI spec, generate, serve, and verify that MCP tool calls work correctly.

**Step 1: Write integration test**

```rust
/// Tests the full pipeline: OpenAPI spec → generate → load → execute script
#[tokio::test]
async fn test_full_roundtrip_with_mock_api() {
    // 1. Generate from petstore spec
    let output_dir = tempfile::tempdir().unwrap();
    code_mcp::codegen::generate::generate(
        &["testdata/petstore.yaml".to_string()],
        output_dir.path(),
    ).unwrap();

    // 2. Load manifest
    let manifest: Manifest = serde_json::from_str(
        &std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap()
    ).unwrap();

    // 3. Create executor with mock HTTP handler
    let handler = HttpHandler::mock(|method, url, _query, _body| {
        // Respond to any GET /pets/{id} with a canned pet
        if method == "GET" && url.contains("/pets/") {
            Ok(serde_json::json!({
                "id": "pet-1",
                "name": "Buddy",
                "status": "available"
            }))
        } else if method == "GET" && url.ends_with("/pets") {
            Ok(serde_json::json!([
                {"id": "pet-1", "name": "Buddy", "status": "available"},
                {"id": "pet-2", "name": "Max", "status": "pending"}
            ]))
        } else {
            Err(anyhow::anyhow!("unexpected: {method} {url}"))
        }
    });

    let executor = ScriptExecutor::new(
        manifest,
        handler,
        ExecutorConfig::default(),
    ).unwrap();

    // 4. Execute a script that chains multiple SDK calls
    let result = executor.execute(r#"
        local pet = sdk.get_pet("pet-1")
        local all = sdk.list_pets()
        return {
            single_name = pet.name,
            total_count = #all,
        }
    "#, &HashMap::new()).await.unwrap();

    assert_eq!(result.result["single_name"], "Buddy");
    assert_eq!(result.result["total_count"], 2);
    assert!(result.stats.api_calls >= 2);
}
```

**Step 2: Run test**

```bash
cargo test --test full_roundtrip
```

**Step 3: Fix any issues found during integration**

**Step 4: Commit**

```bash
git add tests/full_roundtrip.rs
git commit -m "test: full round-trip integration test with mock API"
```

---

## Summary of tasks

| # | Task | Key deliverable |
|---|---|---|
| 1 | Project Init & CLI Skeleton | Cargo.toml, main.rs, cli.rs |
| 2 | Test Data & OpenAPI Parsing | petstore.yaml, parser.rs |
| 3 | Manifest Data Model | manifest.rs with all types |
| 4 | OpenAPI-to-Manifest Conversion | spec_to_manifest() |
| 5 | Lua Annotation Generation | annotations.rs |
| 6 | Generate Subcommand | End-to-end codegen pipeline |
| 7 | Lua Sandbox | Locked-down Lua 5.4 environment |
| 8 | SDK Function Registration | Manifest → Lua functions |
| 9 | HTTP Client with Auth | Auth injection, request building |
| 10 | Script Executor | Timeout, logs, stats |
| 11 | MCP Server + Doc Tools | rmcp-based MCP server |
| 12 | MCP Resources | Browsable SDK docs |
| 13 | Serve & Run Subcommands | Wire everything together |
| 14 | SSE Transport | Remote MCP access |
| 15 | Auth from Environment | Env var credential loading |
| 16 | Dockerfile | Container packaging |
| 17 | Integration Test | Full round-trip validation |
