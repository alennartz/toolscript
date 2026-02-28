use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use super::luau_types::{field_type_to_luau, render_enum_type};
use super::manifest::{FunctionDef, Manifest, McpToolDef, ParamType, SchemaDef};

/// Render a Luau type-annotated documentation block for a single function.
///
/// Produces output like:
/// ```luau
/// -- Get a pet by ID
/// --
/// -- Returns a single pet by its unique identifier.
/// --
/// -- @param pet_id - The pet's unique identifier
/// function sdk.get_pet(params: { pet_id: string }): Pet end
/// ```
///
/// Frozen parameters (those with a `frozen_value`) are excluded from both
/// the `@param` doc comments and the function signature. The signature uses
/// a table-based calling convention with four cases:
///
/// | Visible params? | Body? | Signature                          |
/// |-----------------|-------|------------------------------------|
/// | Yes             | No    | `(params: { ... })`                |
/// | Yes             | Yes   | `(params: { ... }, body: Type)`    |
/// | No              | No    | `()`                               |
/// | No              | Yes   | `(body: Type)`                     |
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

    // Only visible (non-frozen) params get @param doc comments
    let visible_params: Vec<_> = func
        .parameters
        .iter()
        .filter(|p| p.frozen_value.is_none())
        .collect();

    for param in &visible_params {
        if let Some(desc) = &param.description {
            let desc = desc.trim();
            if !desc.is_empty() {
                lines.push(format!("-- @param {} - {desc}", param.name));
            }
        }
    }

    // Request body description
    if let Some(body) = &func.request_body
        && let Some(desc) = &body.description
    {
        let desc = desc.trim();
        if !desc.is_empty() {
            lines.push(format!("-- @param body - {desc}"));
        }
    }

    // Build table entries for visible params
    let has_visible_params = !visible_params.is_empty();

    let table_entries: Vec<String> = visible_params
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

    // Build function args based on calling convention
    let mut args = Vec::new();
    if has_visible_params {
        args.push(format!("params: {{ {} }}", table_entries.join(", ")));
    }
    if let Some(body) = &func.request_body {
        if body.required {
            args.push(format!("body: {}", body.schema));
        } else {
            args.push(format!("body: {}?", body.schema));
        }
    }
    let params_str = args.join(", ");

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

/// Transitively resolve all schema names reachable from the initial set.
///
/// Performs a BFS walk: for each schema name in `initial`, looks it up in
/// `schema_map`, collects type refs from its fields, and recurses. Returns
/// the resolved names in sorted order (only names present in the schema map).
fn resolve_transitive_schemas(
    initial: Vec<String>,
    schema_map: &std::collections::HashMap<&str, &SchemaDef>,
) -> Vec<String> {
    let mut resolved: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue = initial;
    while let Some(name) = queue.pop() {
        if !resolved.insert(name.clone()) {
            continue;
        }
        if let Some(schema) = schema_map.get(name.as_str()) {
            for field in &schema.fields {
                field.field_type.collect_refs(&mut queue);
            }
        }
    }
    let mut sorted: Vec<String> = resolved
        .into_iter()
        .filter(|name| schema_map.contains_key(name.as_str()))
        .collect();
    sorted.sort_unstable();
    sorted
}

/// Render a complete documentation block: function signature + all transitively referenced schemas.
pub fn render_function_docs(func: &FunctionDef, schemas: &[SchemaDef]) -> String {
    let mut output = render_function_annotation(func);

    // Collect directly referenced schema names from response and request body
    let mut needed: Vec<String> = Vec::new();
    if let Some(ref schema) = func.response_schema {
        needed.push(schema.clone());
    }
    if let Some(ref body) = func.request_body {
        needed.push(body.schema.clone());
    }

    let schema_map: std::collections::HashMap<&str, &SchemaDef> =
        schemas.iter().map(|s| (s.name.as_str(), s)).collect();

    for name in resolve_transitive_schemas(needed, &schema_map) {
        output.push('\n');
        output.push('\n');
        output.push_str(&render_schema_annotation(schema_map[name.as_str()]));
    }

    output
}

/// Render a Luau function signature for an MCP tool.
///
/// Produces output like:
/// ```luau
/// -- Read the complete contents of a file
/// -- @param path: string - File path to read
/// -- @param encoding: string? - File encoding
/// function sdk.filesystem.read_file(params: { path: string, encoding: string? }): any end
/// ```
///
/// Rules:
/// - First line: description (if present), prefixed with `-- `
/// - One `-- @param` line per parameter: `-- @param name: type[?] - description`
/// - Optional params get `?` suffix on type
/// - Function signature: `function sdk.<server>.<tool_name>(params: { ... }): any end`
/// - If no params, signature is: `function sdk.<server>.<tool_name>(): any end`
/// - Return type is always `any` (MCP tools return opaque content)
pub fn render_mcp_tool_annotation(tool: &McpToolDef) -> String {
    let mut lines = Vec::new();

    // Description
    if let Some(desc) = &tool.description {
        lines.push(format!("-- {desc}"));
    }

    // @param lines
    for param in &tool.params {
        let optional = if param.required { "" } else { "?" };
        let desc_part = param
            .description
            .as_deref()
            .map_or(String::new(), |d| format!(" - {d}"));
        lines.push(format!(
            "-- @param {}: {}{}{}",
            param.name, param.luau_type, optional, desc_part
        ));
    }

    // Function signature
    if tool.params.is_empty() {
        lines.push(format!(
            "function sdk.{}.{}(): any end",
            tool.server, tool.name
        ));
    } else {
        let param_entries: Vec<String> = tool
            .params
            .iter()
            .map(|p| {
                let optional = if p.required { "" } else { "?" };
                format!("{}: {}{}", p.name, p.luau_type, optional)
            })
            .collect();
        lines.push(format!(
            "function sdk.{}.{}(params: {{ {} }}): any end",
            tool.server,
            tool.name,
            param_entries.join(", ")
        ));
    }

    lines.join("\n")
}

/// Render a complete documentation block for an MCP tool: function signature
/// plus transitively referenced schemas.
///
/// Like [`render_mcp_tool_annotation`] but uses transitive `$ref` resolution
/// (the same algorithm as [`render_function_docs`]) to include only the schemas
/// actually referenced by the tool's parameters, rather than dumping all schemas.
pub fn render_mcp_tool_docs(tool: &McpToolDef) -> String {
    let mut output = render_mcp_tool_annotation(tool);

    let all_schemas: Vec<&SchemaDef> = tool
        .schemas
        .iter()
        .chain(tool.output_schemas.iter())
        .collect();
    let schema_map: std::collections::HashMap<&str, &SchemaDef> =
        all_schemas.iter().map(|s| (s.name.as_str(), *s)).collect();

    let mut needed = Vec::new();
    for param in &tool.params {
        param.field_type.collect_refs(&mut needed);
    }

    for name in resolve_transitive_schemas(needed, &schema_map) {
        output.push_str("\n\n");
        output.push_str(&render_schema_annotation(schema_map[name.as_str()]));
    }

    output
}

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
        let optional_marker = if !field.required || field.nullable {
            "?"
        } else {
            ""
        };
        let mut comment_parts: Vec<String> = Vec::new();
        if let Some(d) = &field.description {
            comment_parts.push(d.trim().to_string());
        }
        if let Some(f) = &field.format {
            comment_parts.push(format!("({f})"));
        }
        let desc = if comment_parts.is_empty() {
            String::new()
        } else {
            format!("  -- {}", comment_parts.join(" "))
        };

        lines.push(format!(
            "    {}: {type_str}{optional_marker},{desc}",
            field.name
        ));
    }

    // Closing brace
    lines.push("}".to_string());

    lines.join("\n")
}

/// Generate annotation files grouped by tag.
///
/// Returns a `Vec<(filename, content)>` where each file corresponds to
/// a tag group (or "default" for untagged functions), plus a `_meta.luau`
/// file with API metadata.
pub fn generate_annotation_files(manifest: &Manifest) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();

    // Group functions by tag
    let mut groups: BTreeMap<String, Vec<&FunctionDef>> = BTreeMap::new();
    for func in &manifest.functions {
        let tag = func.tag.as_deref().unwrap_or("default").to_string();
        groups.entry(tag).or_default().push(func);
    }

    // Build a lookup map for schemas by name
    let schema_map: BTreeMap<&str, &SchemaDef> = manifest
        .schemas
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();

    // For each tag group, produce a .luau file
    for (tag, funcs) in &groups {
        let mut content = String::new();

        // Header comment with API metadata
        for api in &manifest.apis {
            let _ = write!(content, "-- {} API", api.name);
            if let Some(version) = &api.version {
                let _ = write!(content, " v{version}");
            }
            content.push('\n');
            if let Some(description) = &api.description {
                let desc = description.trim();
                if !desc.is_empty() {
                    let _ = writeln!(content, "-- {desc}");
                }
            }
        }
        content.push('\n');

        // Collect all schema names referenced by functions in this group
        let mut referenced_schemas: BTreeSet<&str> = BTreeSet::new();
        for func in funcs {
            if let Some(ref schema) = func.response_schema {
                referenced_schemas.insert(schema.as_str());
            }
            if let Some(ref body) = func.request_body {
                referenced_schemas.insert(body.schema.as_str());
            }
        }

        // Render function annotations
        for (i, func) in funcs.iter().enumerate() {
            if i > 0 {
                content.push('\n');
            }
            content.push_str(&render_function_annotation(func));
            content.push('\n');
        }

        // Render schema annotations referenced by functions in this group
        for schema_name in &referenced_schemas {
            if let Some(schema) = schema_map.get(schema_name) {
                content.push('\n');
                content.push_str(&render_schema_annotation(schema));
                content.push('\n');
            }
        }

        files.push((format!("{tag}.luau"), content));
    }

    // Generate _meta.luau with API metadata
    let mut meta_content = String::new();
    meta_content.push_str("-- API Metadata\n");
    meta_content.push_str("-- Generated by toolscript\n\n");
    for api in &manifest.apis {
        let _ = writeln!(meta_content, "-- API: {}", api.name);
        if let Some(version) = &api.version {
            let _ = writeln!(meta_content, "-- Version: {version}");
        }
        if let Some(description) = &api.description {
            let desc = description.trim();
            if !desc.is_empty() {
                let _ = writeln!(meta_content, "-- Description: {desc}");
            }
        }
        let _ = writeln!(meta_content, "-- Base URL: {}", api.base_url);
        meta_content.push('\n');
    }
    files.push(("_meta.luau".to_string(), meta_content));

    files
}

/// Convert a `ParamType` to its Luau type name.
fn param_type_to_luau(param_type: &ParamType) -> String {
    match param_type {
        ParamType::String => "string".to_string(),
        ParamType::Integer | ParamType::Number => "number".to_string(),
        ParamType::Boolean => "boolean".to_string(),
    }
}

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
                format: None,
                frozen_value: None,
            }],
            request_body: None,
            response_schema: Some("Pet".to_string()),
        };

        let output = render_function_annotation(&func);
        assert!(
            output.contains("-- Get a pet by ID"),
            "Missing summary. Got:\n{output}"
        );
        assert!(
            output.contains("-- Returns a single pet by its unique identifier."),
            "Missing description. Got:\n{output}"
        );
        assert!(
            output.contains("-- @param pet_id - The pet's unique identifier"),
            "Missing @param. Got:\n{output}"
        );
        assert!(
            output.contains("function sdk.get_pet(params: { pet_id: string }): Pet end"),
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
            output.contains("status: string?"),
            "Optional param missing ? suffix. Got:\n{output}"
        );
        assert!(
            output.contains("limit: number"),
            "Required param should use number type. Got:\n{output}"
        );
        assert!(
            !output.contains("limit: number?"),
            "Required param should NOT have ?. Got:\n{output}"
        );
        assert!(
            output.contains(
                "function sdk.list_pets(params: { status: string?, limit: number }): Pet end"
            ),
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
                format: None,
                frozen_value: None,
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
                    nullable: false,
                    format: None,
                },
                FieldDef {
                    name: "name".to_string(),
                    field_type: FieldType::String,
                    required: true,
                    description: Some("The pet's name".to_string()),
                    enum_values: None,
                    nullable: false,
                    format: None,
                },
                FieldDef {
                    name: "tags".to_string(),
                    field_type: FieldType::Array {
                        items: Box::new(FieldType::String),
                    },
                    required: false,
                    description: Some("Classification tags".to_string()),
                    enum_values: None,
                    nullable: false,
                    format: None,
                },
                FieldDef {
                    name: "owner".to_string(),
                    field_type: FieldType::Object {
                        schema: "User".to_string(),
                    },
                    required: false,
                    description: Some("The pet's owner".to_string()),
                    enum_values: None,
                    nullable: false,
                    format: None,
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
        assert!(
            output.contains('}'),
            "Missing closing brace. Got:\n{output}"
        );
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
                    nullable: false,
                    format: None,
                },
                FieldDef {
                    name: "label".to_string(),
                    field_type: FieldType::String,
                    required: false,
                    description: None,
                    enum_values: None,
                    nullable: false,
                    format: None,
                },
            ],
        };

        let output = render_schema_annotation(&schema);
        assert!(
            output.contains("    id: string,"),
            "Required field should not have ?. Got:\n{output}"
        );
        assert!(
            !output.contains("id: string?,"),
            "Required field should NOT have ?. Got:\n{output}"
        );
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
                nullable: false,
                format: None,
            }],
        };

        let output = render_schema_annotation(&schema);
        assert!(
            output
                .contains(r#"    status: ("available" | "pending" | "sold"),  -- Current status"#),
            "Enum field should use Luau union type. Got:\n{output}"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
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
                        nullable: false,
                        format: None,
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
                        nullable: false,
                        format: None,
                    }],
                },
            ],
            mcp_servers: vec![],
        };

        let files = generate_annotation_files(&manifest);

        assert!(
            files.len() >= 2,
            "Expected at least 2 files, got {}",
            files.len()
        );

        for (filename, _) in &files {
            assert!(
                std::path::Path::new(filename)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("luau")),
                "File {filename} doesn't end in .luau"
            );
        }

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

        let meta_file = files.iter().find(|(name, _)| name == "_meta.luau");
        assert!(meta_file.is_some(), "Missing _meta.luau");
        let meta_content = &meta_file.unwrap().1;
        assert!(
            meta_content.contains("petstore"),
            "_meta.luau missing API name"
        );
        assert!(meta_content.contains("1.0.0"), "_meta.luau missing version");
    }

    #[test]
    fn test_render_nullable_field() {
        let schema = SchemaDef {
            name: "Item".to_string(),
            description: None,
            fields: vec![
                FieldDef {
                    name: "name".to_string(),
                    field_type: FieldType::String,
                    required: true,
                    description: None,
                    enum_values: None,
                    nullable: false,
                    format: None,
                },
                FieldDef {
                    name: "deleted_at".to_string(),
                    field_type: FieldType::String,
                    required: true,
                    description: None,
                    enum_values: None,
                    nullable: true,
                    format: Some("date-time".to_string()),
                },
            ],
        };

        let output = render_schema_annotation(&schema);
        assert!(
            output.contains("deleted_at: string?,"),
            "Nullable required field should have ?. Got:\n{output}"
        );
        assert!(
            output.contains("name: string,"),
            "Non-nullable required field should NOT have ?. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_format_comment() {
        let schema = SchemaDef {
            name: "Item".to_string(),
            description: None,
            fields: vec![FieldDef {
                name: "id".to_string(),
                field_type: FieldType::String,
                required: true,
                description: Some("Unique ID".to_string()),
                enum_values: None,
                nullable: false,
                format: Some("uuid".to_string()),
            }],
        };

        let output = render_schema_annotation(&schema);
        assert!(
            output.contains("(uuid)"),
            "Format should appear in comment. Got:\n{output}"
        );
        assert!(
            output.contains("Unique ID"),
            "Description should still appear. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_map_field_type() {
        let schema = SchemaDef {
            name: "Config".to_string(),
            description: None,
            fields: vec![FieldDef {
                name: "metadata".to_string(),
                field_type: FieldType::Map {
                    value: Box::new(FieldType::String),
                },
                required: true,
                description: Some("Key-value pairs".to_string()),
                enum_values: None,
                nullable: false,
                format: None,
            }],
        };

        let output = render_schema_annotation(&schema);
        assert!(
            output.contains("[string]: string"),
            "Map type should render with [string]: string. Got:\n{output}"
        );
    }

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
            output.contains(
                "function sdk.list_pets(params: { status: string?, limit: number }): Pet end"
            ),
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
            output.contains(
                "function sdk.create_pet(params: { tag: string? }, body: NewPet): Pet end"
            ),
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

    #[test]
    fn test_render_mcp_tool_annotation_basic() {
        let tool = McpToolDef {
            name: "read_file".to_string(),
            server: "filesystem".to_string(),
            description: Some("Read a file from disk".to_string()),
            params: vec![
                McpParamDef {
                    name: "path".to_string(),
                    luau_type: "string".to_string(),
                    required: true,
                    description: Some("File path to read".to_string()),
                    ..Default::default()
                },
                McpParamDef {
                    name: "encoding".to_string(),
                    luau_type: "string".to_string(),
                    required: false,
                    description: Some("File encoding".to_string()),
                    ..Default::default()
                },
            ],
            schemas: vec![],
            output_schemas: vec![],
        };
        let output = render_mcp_tool_annotation(&tool);
        assert!(
            output.contains("-- Read a file from disk"),
            "Missing description. Got:\n{output}"
        );
        assert!(
            output.contains("-- @param path: string - File path to read"),
            "Missing path param. Got:\n{output}"
        );
        assert!(
            output.contains("-- @param encoding: string? - File encoding"),
            "Missing optional encoding param. Got:\n{output}"
        );
        assert!(
            output.contains("function sdk.filesystem.read_file(params: {"),
            "Missing function sig. Got:\n{output}"
        );
        assert!(
            output.contains("path: string"),
            "Missing path in sig. Got:\n{output}"
        );
        assert!(
            output.contains("encoding: string?"),
            "Missing encoding in sig. Got:\n{output}"
        );
        assert!(
            output.contains("): any end"),
            "Missing return type. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_mcp_tool_annotation_no_params() {
        let tool = McpToolDef {
            name: "list_tools".to_string(),
            server: "meta".to_string(),
            description: None,
            params: vec![],
            schemas: vec![],
            output_schemas: vec![],
        };
        let output = render_mcp_tool_annotation(&tool);
        assert!(
            output.contains("function sdk.meta.list_tools(): any end"),
            "Expected no-param sig. Got:\n{output}"
        );
        // No description line
        assert!(
            !output.contains("-- @param"),
            "Should have no param lines. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_mcp_tool_docs_includes_schemas() {
        let tool = McpToolDef {
            name: "create_user".to_string(),
            server: "users".to_string(),
            description: Some("Create a user".to_string()),
            params: vec![McpParamDef {
                name: "data".to_string(),
                luau_type: "UserInput".to_string(),
                required: true,
                description: None,
                field_type: FieldType::Object {
                    schema: "UserInput".to_string(),
                },
            }],
            schemas: vec![SchemaDef {
                name: "UserInput".to_string(),
                description: Some("User creation data".to_string()),
                fields: vec![FieldDef {
                    name: "name".to_string(),
                    field_type: FieldType::String,
                    required: true,
                    description: None,
                    enum_values: None,
                    nullable: false,
                    format: None,
                }],
            }],
            output_schemas: vec![],
        };
        let output = render_mcp_tool_docs(&tool);
        assert!(
            output.contains("function sdk.users.create_user"),
            "Missing function sig. Got:\n{output}"
        );
        assert!(
            output.contains("export type UserInput"),
            "Missing schema. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_mcp_tool_docs_transitive_refs() {
        // A → B → C chain: only referenced schemas should appear
        let tool = McpToolDef {
            name: "create".to_string(),
            server: "svc".to_string(),
            description: None,
            params: vec![McpParamDef {
                name: "input".to_string(),
                luau_type: "A".to_string(),
                required: true,
                description: None,
                field_type: FieldType::Object {
                    schema: "A".to_string(),
                },
            }],
            schemas: vec![
                SchemaDef {
                    name: "A".to_string(),
                    description: None,
                    fields: vec![FieldDef {
                        name: "b_ref".to_string(),
                        field_type: FieldType::Object {
                            schema: "B".to_string(),
                        },
                        required: true,
                        description: None,
                        enum_values: None,
                        nullable: false,
                        format: None,
                    }],
                },
                SchemaDef {
                    name: "B".to_string(),
                    description: None,
                    fields: vec![FieldDef {
                        name: "c_ref".to_string(),
                        field_type: FieldType::Object {
                            schema: "C".to_string(),
                        },
                        required: true,
                        description: None,
                        enum_values: None,
                        nullable: false,
                        format: None,
                    }],
                },
                SchemaDef {
                    name: "C".to_string(),
                    description: None,
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
                // This schema should NOT appear (not referenced)
                SchemaDef {
                    name: "Unused".to_string(),
                    description: None,
                    fields: vec![],
                },
            ],
            output_schemas: vec![],
        };

        let output = render_mcp_tool_docs(&tool);
        assert!(
            output.contains("export type A"),
            "Missing A schema. Got:\n{output}"
        );
        assert!(
            output.contains("export type B"),
            "Missing B schema. Got:\n{output}"
        );
        assert!(
            output.contains("export type C"),
            "Missing C schema. Got:\n{output}"
        );
        assert!(
            !output.contains("export type Unused"),
            "Unused schema should NOT appear. Got:\n{output}"
        );
    }

    #[test]
    fn test_render_mcp_tool_docs_no_field_type_no_schemas() {
        // When params have default FieldType::String, no schemas should be emitted
        let tool = McpToolDef {
            name: "echo".to_string(),
            server: "test".to_string(),
            description: None,
            params: vec![McpParamDef {
                name: "text".to_string(),
                luau_type: "string".to_string(),
                required: true,
                description: None,
                ..Default::default()
            }],
            schemas: vec![SchemaDef {
                name: "SomeUnused".to_string(),
                description: None,
                fields: vec![],
            }],
            output_schemas: vec![],
        };
        let output = render_mcp_tool_docs(&tool);
        assert!(
            !output.contains("export type"),
            "Should not include unreferenced schemas. Got:\n{output}"
        );
    }
}
