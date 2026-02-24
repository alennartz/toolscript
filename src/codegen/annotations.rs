use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use super::manifest::{FieldType, FunctionDef, Manifest, ParamType, SchemaDef};

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
    if let Some(body) = &func.request_body
        && let Some(desc) = &body.description
    {
        let desc = desc.trim();
        if !desc.is_empty() {
            lines.push(format!("-- @param body - {desc}"));
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
    meta_content.push_str("-- Generated by code-mcp\n\n");
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

/// Convert a `FieldType` to its Luau type name.
fn field_type_to_luau(field_type: &FieldType) -> String {
    match field_type {
        FieldType::String => "string".to_string(),
        FieldType::Integer | FieldType::Number => "number".to_string(),
        FieldType::Boolean => "boolean".to_string(),
        FieldType::Array { items } => format!("{{{}}}", field_type_to_luau(items)),
        FieldType::Object { schema } => schema.clone(),
        FieldType::Map { value } => format!("{{ [string]: {} }}", field_type_to_luau(value)),
    }
}

/// Render an enum type as a Luau literal union: `"val1" | "val2" | "val3"`.
fn render_enum_type(values: &[String]) -> String {
    let inner = values
        .iter()
        .map(|v| format!("\"{v}\""))
        .collect::<Vec<_>>()
        .join(" | ");
    format!("({inner})")
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
}
