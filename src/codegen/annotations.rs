use std::collections::{BTreeMap, BTreeSet};

use super::manifest::{FieldType, FunctionDef, Manifest, ParamType, SchemaDef};

/// Render a LuaLS-compatible annotation block for a single function.
///
/// Produces output like:
/// ```lua
/// --- Get a pet by ID
/// ---
/// --- Returns a single pet by its unique identifier.
/// ---
/// --- @param pet_id string The pet's unique identifier
/// --- @return Pet
/// function sdk.get_pet(pet_id) end
/// ```
pub fn render_function_annotation(func: &FunctionDef) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Summary line
    if let Some(summary) = &func.summary {
        lines.push(format!("--- {}", summary.trim()));
    }

    // Description block (separated by blank doc line)
    if let Some(description) = &func.description {
        let desc = description.trim();
        if !desc.is_empty() {
            lines.push("---".to_string());
            for desc_line in desc.lines() {
                let trimmed = desc_line.trim();
                if trimmed.is_empty() {
                    lines.push("---".to_string());
                } else {
                    lines.push(format!("--- {trimmed}"));
                }
            }
        }
    }

    // Deprecated annotation
    if func.deprecated {
        lines.push("--- @deprecated".to_string());
    }

    // Parameter annotations
    for param in &func.parameters {
        let optional_marker = if param.required { "" } else { "?" };
        let type_str = if let Some(enum_values) = &param.enum_values {
            render_enum_type(enum_values)
        } else {
            param_type_to_lua(&param.param_type)
        };
        let desc = param
            .description
            .as_deref()
            .map(|d| format!(" {}", d.trim()))
            .unwrap_or_default();
        lines.push(format!(
            "--- @param {}{} {}{}",
            param.name, optional_marker, type_str, desc
        ));
    }

    // Request body as an additional parameter
    if let Some(body) = &func.request_body {
        let optional_marker = if body.required { "" } else { "?" };
        let desc = body
            .description
            .as_deref()
            .map(|d| format!(" {}", d.trim()))
            .unwrap_or_default();
        lines.push(format!(
            "--- @param body{} {}{}",
            optional_marker, body.schema, desc
        ));
    }

    // Return type
    if let Some(response_schema) = &func.response_schema {
        lines.push(format!("--- @return {response_schema}"));
    }

    // Function signature
    let mut param_names: Vec<&str> = func
        .parameters
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    if func.request_body.is_some() {
        param_names.push("body");
    }
    let params_str = param_names.join(", ");
    lines.push(format!("function sdk.{}({}) end", func.name, params_str));

    lines.join("\n")
}

/// Render a LuaLS-compatible annotation block for a schema (class).
///
/// Produces output like:
/// ```lua
/// --- A pet in the store
/// --- @class Pet
/// --- @field id string Unique ID
/// --- @field name string The pet's name
/// --- @field status? "available"|"pending"|"sold" Current status
/// ```
pub fn render_schema_annotation(schema: &SchemaDef) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Description line before @class
    if let Some(description) = &schema.description {
        let desc = description.trim();
        if !desc.is_empty() {
            lines.push(format!("--- {desc}"));
        }
    }

    // @class annotation
    lines.push(format!("--- @class {}", schema.name));

    // Field annotations
    for field in &schema.fields {
        let optional_marker = if field.required { "" } else { "?" };
        let type_str = if let Some(enum_values) = &field.enum_values {
            render_enum_type(enum_values)
        } else {
            field_type_to_lua(&field.field_type)
        };
        let desc = field
            .description
            .as_deref()
            .map(|d| format!(" {}", d.trim()))
            .unwrap_or_default();
        lines.push(format!(
            "--- @field {}{} {}{}",
            field.name, optional_marker, type_str, desc
        ));
    }

    lines.join("\n")
}

/// Generate annotation files grouped by tag.
///
/// Returns a `Vec<(filename, content)>` where each file corresponds to
/// a tag group (or "default" for untagged functions), plus a `_meta.lua`
/// file with API metadata.
pub fn generate_annotation_files(manifest: &Manifest) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();

    // Group functions by tag
    let mut groups: BTreeMap<String, Vec<&FunctionDef>> = BTreeMap::new();
    for func in &manifest.functions {
        let tag = func
            .tag
            .as_deref()
            .unwrap_or("default")
            .to_string();
        groups.entry(tag).or_default().push(func);
    }

    // Build a lookup map for schemas by name
    let schema_map: BTreeMap<&str, &SchemaDef> = manifest
        .schemas
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();

    // For each tag group, produce a .lua file
    for (tag, funcs) in &groups {
        let mut content = String::new();

        // Header comment with API metadata
        for api in &manifest.apis {
            content.push_str(&format!("-- {} API", api.name));
            if let Some(version) = &api.version {
                content.push_str(&format!(" v{version}"));
            }
            content.push('\n');
            if let Some(description) = &api.description {
                let desc = description.trim();
                if !desc.is_empty() {
                    content.push_str(&format!("-- {desc}\n"));
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

        files.push((format!("{tag}.lua"), content));
    }

    // Generate _meta.lua with API metadata
    let mut meta_content = String::new();
    meta_content.push_str("-- API Metadata\n");
    meta_content.push_str("-- Generated by code-mcp\n\n");
    for api in &manifest.apis {
        meta_content.push_str(&format!("-- API: {}\n", api.name));
        if let Some(version) = &api.version {
            meta_content.push_str(&format!("-- Version: {version}\n"));
        }
        if let Some(description) = &api.description {
            let desc = description.trim();
            if !desc.is_empty() {
                meta_content.push_str(&format!("-- Description: {desc}\n"));
            }
        }
        meta_content.push_str(&format!("-- Base URL: {}\n", api.base_url));
        meta_content.push('\n');
    }
    files.push(("_meta.lua".to_string(), meta_content));

    files
}

/// Convert a ParamType to its Lua type name.
fn param_type_to_lua(param_type: &ParamType) -> String {
    match param_type {
        ParamType::String => "string".to_string(),
        ParamType::Integer => "integer".to_string(),
        ParamType::Number => "number".to_string(),
        ParamType::Boolean => "boolean".to_string(),
    }
}

/// Convert a FieldType to its Lua type name.
fn field_type_to_lua(field_type: &FieldType) -> String {
    match field_type {
        FieldType::String => "string".to_string(),
        FieldType::Integer => "integer".to_string(),
        FieldType::Number => "number".to_string(),
        FieldType::Boolean => "boolean".to_string(),
        FieldType::Array { items } => format!("{}[]", field_type_to_lua(items)),
        FieldType::Object { schema } => schema.clone(),
    }
}

/// Render an enum type as a Lua literal union: `"val1"|"val2"|"val3"`.
fn render_enum_type(values: &[String]) -> String {
    values
        .iter()
        .map(|v| format!("\"{v}\""))
        .collect::<Vec<_>>()
        .join("|")
}

#[cfg(test)]
mod tests {
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
        assert!(output.contains("--- Get a pet by ID"), "Missing summary");
        assert!(
            output.contains("--- Returns a single pet by its unique identifier."),
            "Missing description"
        );
        assert!(
            output.contains("--- @param pet_id string The pet's unique identifier"),
            "Missing @param"
        );
        assert!(output.contains("--- @return Pet"), "Missing @return");
        assert!(
            output.contains("function sdk.get_pet(pet_id) end"),
            "Missing function signature"
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
        // Optional param should have ? suffix
        assert!(
            output.contains("--- @param status? string Filter by status"),
            "Optional param missing ? suffix. Got:\n{output}"
        );
        // Required param should NOT have ? suffix
        assert!(
            output.contains("--- @param limit integer Max items"),
            "Required param should not have ? suffix. Got:\n{output}"
        );
        assert!(
            output.contains("function sdk.list_pets(status, limit) end"),
            "Missing function signature. Got:\n{output}"
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
            output.contains(r#"--- @param status? "available"|"pending"|"sold" Filter by status"#),
            "Enum param should use literal union type. Got:\n{output}"
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
            output.contains("--- @deprecated"),
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
            output.contains("--- @param body NewPet The pet to create"),
            "Missing body param. Got:\n{output}"
        );
        assert!(
            output.contains("function sdk.create_pet(body) end"),
            "Missing body in function signature. Got:\n{output}"
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
            output.contains("--- A pet in the store"),
            "Missing description. Got:\n{output}"
        );
        assert!(
            output.contains("--- @class Pet"),
            "Missing @class. Got:\n{output}"
        );
        assert!(
            output.contains("--- @field id string Unique ID"),
            "Missing id field. Got:\n{output}"
        );
        assert!(
            output.contains("--- @field name string The pet's name"),
            "Missing name field. Got:\n{output}"
        );
        assert!(
            output.contains("--- @field tags? string[] Classification tags"),
            "Missing array field. Got:\n{output}"
        );
        assert!(
            output.contains("--- @field owner? User The pet's owner"),
            "Missing object field. Got:\n{output}"
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
        // Required field: no ? suffix
        assert!(
            output.contains("--- @field id string"),
            "Required field should not have ?. Got:\n{output}"
        );
        // Check it doesn't have a ? (we need to be more precise)
        assert!(
            !output.contains("--- @field id? string"),
            "Required field should NOT have ?. Got:\n{output}"
        );
        // Optional field: has ? suffix
        assert!(
            output.contains("--- @field label? string"),
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
            output.contains(r#"--- @field status "available"|"pending"|"sold" Current status"#),
            "Enum field should use literal union type. Got:\n{output}"
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

        // Should have: pets.lua + _meta.lua
        assert!(
            files.len() >= 2,
            "Expected at least 2 files, got {}",
            files.len()
        );

        // All filenames should end in .lua
        for (filename, _) in &files {
            assert!(
                filename.ends_with(".lua"),
                "File {filename} doesn't end in .lua"
            );
        }

        // Check pets.lua exists and has content
        let pets_file = files.iter().find(|(name, _)| name == "pets.lua");
        assert!(pets_file.is_some(), "Missing pets.lua");
        let pets_content = &pets_file.unwrap().1;
        assert!(
            !pets_content.is_empty(),
            "pets.lua is empty"
        );
        assert!(
            pets_content.contains("function sdk.list_pets"),
            "pets.lua missing list_pets function"
        );
        assert!(
            pets_content.contains("function sdk.create_pet"),
            "pets.lua missing create_pet function"
        );
        assert!(
            pets_content.contains("@class Pet"),
            "pets.lua missing Pet schema"
        );
        assert!(
            pets_content.contains("@class NewPet"),
            "pets.lua missing NewPet schema"
        );

        // Check _meta.lua exists
        let meta_file = files.iter().find(|(name, _)| name == "_meta.lua");
        assert!(meta_file.is_some(), "Missing _meta.lua");
        let meta_content = &meta_file.unwrap().1;
        assert!(
            meta_content.contains("petstore"),
            "_meta.lua missing API name"
        );
        assert!(
            meta_content.contains("1.0.0"),
            "_meta.lua missing version"
        );
    }
}
