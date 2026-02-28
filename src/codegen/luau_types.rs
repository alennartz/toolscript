//! Shared JSON Schema to Luau type conversion utilities.
//!
//! Used by both the `OpenAPI` codegen path and MCP tool schema conversion.

use serde_json::Value;

use super::manifest::{FieldDef, FieldType, McpParamDef, SchemaDef};

/// Convert a JSON Schema object (with `properties` / `required`) into a list of
/// [`McpParamDef`] entries suitable for MCP tool parameter metadata.
pub fn json_schema_to_params(schema: &Value) -> Vec<McpParamDef> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };

    let required_set: std::collections::HashSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut params: Vec<McpParamDef> = properties
        .iter()
        .map(|(name, prop)| {
            let field_type = json_schema_prop_to_field_type(prop);
            let enum_values = extract_json_schema_enum(prop);
            let luau_type = enum_values.as_ref().map_or_else(
                || field_type_to_luau(&field_type),
                |ev| render_enum_type(ev),
            );
            let description = prop
                .get("description")
                .and_then(Value::as_str)
                .map(String::from);

            McpParamDef {
                name: name.clone(),
                luau_type,
                required: required_set.contains(name.as_str()),
                description,
                field_type,
            }
        })
        .collect();

    // Sort for deterministic output.
    params.sort_by(|a, b| a.name.cmp(&b.name));
    params
}

/// Convert a single JSON Schema property into a complete [`FieldDef`].
///
/// This is the canonical conversion point for JSON Schema → `FieldDef`, used by
/// both the MCP path (raw `serde_json::Value`) and the `OpenAPI` path (which
/// serialises `openapiv3` types into JSON first).
pub fn json_schema_prop_to_field_def(name: &str, prop: &Value, required: bool) -> FieldDef {
    FieldDef {
        name: name.to_string(),
        field_type: json_schema_prop_to_field_type(prop),
        required,
        description: prop
            .get("description")
            .and_then(Value::as_str)
            .map(String::from),
        enum_values: extract_json_schema_enum(prop),
        nullable: is_json_schema_nullable(prop),
        format: extract_json_schema_format(prop),
    }
}

/// Extract string enum values from a JSON Schema property's `"enum"` array.
fn extract_json_schema_enum(prop: &Value) -> Option<Vec<String>> {
    let arr = prop.get("enum").and_then(Value::as_array)?;
    let values: Vec<String> = arr
        .iter()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

/// Check whether a JSON Schema property is nullable.
///
/// Supports both `OpenAPI` 3.0 (`"nullable": true`) and JSON Schema 2020-12
/// (`"type": ["string", "null"]`).
fn is_json_schema_nullable(prop: &Value) -> bool {
    // OpenAPI 3.0 style
    if prop.get("nullable").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    // JSON Schema 2020-12 style: "type" is an array containing "null"
    if let Some(arr) = prop.get("type").and_then(Value::as_array) {
        return arr.iter().any(|v| v.as_str() == Some("null"));
    }
    false
}

/// Extract the `"format"` string from a JSON Schema property.
fn extract_json_schema_format(prop: &Value) -> Option<String> {
    prop.get("format").and_then(Value::as_str).map(String::from)
}

/// Convert a single JSON Schema property value into a [`FieldType`].
///
/// Handles `$ref`, primitive types, arrays, and objects (with or without
/// explicit `properties`).
pub fn json_schema_prop_to_field_type(prop: &Value) -> FieldType {
    // Handle $ref
    if let Some(ref_str) = prop.get("$ref").and_then(Value::as_str) {
        let schema_name = ref_str.rsplit('/').next().unwrap_or(ref_str).to_string();
        return FieldType::Object {
            schema: schema_name,
        };
    }

    // "type" may be a string or an array (JSON Schema 2020-12 nullable style).
    let type_str = prop
        .get("type")
        .and_then(|v| {
            v.as_str().map(std::borrow::Cow::Borrowed).or_else(|| {
                v.as_array().and_then(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .find(|t| *t != "null")
                        .map(|s| std::borrow::Cow::Owned(s.to_string()))
                })
            })
        })
        .unwrap_or(std::borrow::Cow::Borrowed(""));

    match type_str.as_ref() {
        "integer" => FieldType::Integer,
        "number" => FieldType::Number,
        "boolean" => FieldType::Boolean,
        "array" => {
            let items_type = prop
                .get("items")
                .map_or(FieldType::String, json_schema_prop_to_field_type);
            FieldType::Array {
                items: Box::new(items_type),
            }
        }
        "object" => object_field_type(prop),
        // "string" and unknown types both fall back to String.
        _ => FieldType::String,
    }
}

/// Build a [`FieldType`] for a JSON Schema `"object"` type, distinguishing
/// between objects with explicit `properties` ([`FieldType::InlineObject`]) and
/// bare objects ([`FieldType::Map`]).
///
/// When `properties` is present, `additionalProperties` is intentionally
/// ignored — the object is treated as a struct with known fields.
fn object_field_type(prop: &Value) -> FieldType {
    let Some(properties) = prop.get("properties").and_then(Value::as_object) else {
        // No explicit properties — check additionalProperties for map value type.
        return additional_properties_to_map(prop);
    };

    let required_set: std::collections::HashSet<&str> = prop
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut fields: Vec<FieldDef> = properties
        .iter()
        .map(|(name, fprop)| {
            json_schema_prop_to_field_def(name, fprop, required_set.contains(name.as_str()))
        })
        .collect();

    fields.sort_by(|a, b| a.name.cmp(&b.name));
    FieldType::InlineObject { fields }
}

/// Derive a [`FieldType::Map`] from the `additionalProperties` key of a JSON
/// Schema object, falling back to `Map { value: String }`.
fn additional_properties_to_map(prop: &Value) -> FieldType {
    if let Some(ap) = prop.get("additionalProperties")
        && let Some(obj) = ap.as_object()
    {
        let value_type = json_schema_prop_to_field_type(&Value::Object(obj.clone()));
        return FieldType::Map {
            value: Box::new(value_type),
        };
    }
    FieldType::Map {
        value: Box::new(FieldType::String),
    }
}

/// Extract named schema definitions from `$defs` or `definitions` in a JSON
/// Schema document, converting each into a [`SchemaDef`].
pub fn extract_schema_defs(schema: &Value) -> Vec<SchemaDef> {
    let defs_obj = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .and_then(Value::as_object);

    let Some(defs_obj) = defs_obj else {
        return Vec::new();
    };

    let mut defs: Vec<SchemaDef> = defs_obj
        .iter()
        .map(|(name, def)| {
            let required_set: std::collections::HashSet<&str> = def
                .get("required")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();

            let mut fields: Vec<FieldDef> = def
                .get("properties")
                .and_then(Value::as_object)
                .map(|props| {
                    props
                        .iter()
                        .map(|(fname, fprop)| {
                            json_schema_prop_to_field_def(
                                fname,
                                fprop,
                                required_set.contains(fname.as_str()),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();

            fields.sort_by(|a, b| a.name.cmp(&b.name));

            SchemaDef {
                name: name.clone(),
                description: def
                    .get("description")
                    .and_then(Value::as_str)
                    .map(String::from),
                fields,
            }
        })
        .collect();

    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

/// Convert a [`FieldType`] to its Luau type name.
///
/// This is the canonical deep conversion used by both the `OpenAPI` annotation
/// path and MCP tool annotations.
pub fn field_type_to_luau(field_type: &FieldType) -> String {
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

/// Render an enum type as a Luau literal union: `"val1" | "val2" | "val3"`.
pub fn render_enum_type(values: &[String]) -> String {
    let inner = values
        .iter()
        .map(|v| format!("\"{v}\""))
        .collect::<Vec<_>>()
        .join(" | ");
    format!("({inner})")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

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
            FieldType::Array {
                items: Box::new(FieldType::Integer)
            }
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
            FieldType::Object {
                schema: "User".to_string()
            }
        );
    }

    #[test]
    fn test_enum_extraction() {
        let prop = serde_json::json!({
            "type": "string",
            "enum": ["active", "inactive", "pending"]
        });
        let field = json_schema_prop_to_field_def("status", &prop, true);
        assert_eq!(
            field.enum_values,
            Some(vec![
                "active".to_string(),
                "inactive".to_string(),
                "pending".to_string(),
            ])
        );
        assert_eq!(field.field_type, FieldType::String);
    }

    #[test]
    fn test_nullable_openapi_style() {
        let prop = serde_json::json!({
            "type": "string",
            "nullable": true
        });
        let field = json_schema_prop_to_field_def("name", &prop, true);
        assert!(field.nullable);
    }

    #[test]
    fn test_nullable_json_schema_2020_12_style() {
        let prop = serde_json::json!({
            "type": ["string", "null"]
        });
        let field = json_schema_prop_to_field_def("name", &prop, true);
        assert!(field.nullable);
        // Should resolve the non-null type
        assert_eq!(field.field_type, FieldType::String);
    }

    #[test]
    fn test_format_extraction() {
        let prop = serde_json::json!({
            "type": "string",
            "format": "date-time"
        });
        let field = json_schema_prop_to_field_def("created_at", &prop, true);
        assert_eq!(field.format.as_deref(), Some("date-time"));
    }

    #[test]
    fn test_additional_properties_object() {
        let prop = serde_json::json!({
            "type": "object",
            "additionalProperties": { "type": "integer" }
        });
        assert_eq!(
            json_schema_prop_to_field_type(&prop),
            FieldType::Map {
                value: Box::new(FieldType::Integer),
            }
        );
    }

    #[test]
    fn test_additional_properties_boolean() {
        let prop = serde_json::json!({
            "type": "object",
            "additionalProperties": true
        });
        assert_eq!(
            json_schema_prop_to_field_type(&prop),
            FieldType::Map {
                value: Box::new(FieldType::String),
            }
        );
    }

    #[test]
    fn test_additional_properties_ref() {
        let prop = serde_json::json!({
            "type": "object",
            "additionalProperties": { "$ref": "#/components/schemas/Item" }
        });
        assert_eq!(
            json_schema_prop_to_field_type(&prop),
            FieldType::Map {
                value: Box::new(FieldType::Object {
                    schema: "Item".to_string(),
                }),
            }
        );
    }

    #[test]
    fn test_openapi_serialization_bridge() {
        // Verify that openapiv3 Schema serialization produces JSON that
        // json_schema_prop_to_field_def can consume correctly.
        use openapiv3::{Schema, SchemaData, SchemaKind, StringType, Type};

        let schema = Schema {
            schema_data: SchemaData {
                description: Some("A status field".to_string()),
                nullable: true,
                ..Default::default()
            },
            schema_kind: SchemaKind::Type(Type::String(StringType {
                enumeration: vec![Some("active".to_string()), Some("inactive".to_string())],
                format: openapiv3::VariantOrUnknownOrEmpty::Unknown("custom-fmt".to_string()),
                ..Default::default()
            })),
        };

        let json_value = serde_json::to_value(&schema).unwrap();
        let field = json_schema_prop_to_field_def("status", &json_value, true);

        assert_eq!(field.name, "status");
        assert_eq!(field.field_type, FieldType::String);
        assert!(field.required);
        assert_eq!(field.description.as_deref(), Some("A status field"));
        assert_eq!(
            field.enum_values,
            Some(vec!["active".to_string(), "inactive".to_string()])
        );
        assert!(field.nullable);
        assert_eq!(field.format.as_deref(), Some("custom-fmt"));
    }

    #[test]
    fn test_field_type_to_luau_basic() {
        assert_eq!(field_type_to_luau(&FieldType::String), "string");
        assert_eq!(field_type_to_luau(&FieldType::Integer), "number");
        assert_eq!(field_type_to_luau(&FieldType::Number), "number");
        assert_eq!(field_type_to_luau(&FieldType::Boolean), "boolean");
    }

    #[test]
    fn test_field_type_to_luau_array() {
        let arr = FieldType::Array {
            items: Box::new(FieldType::Integer),
        };
        assert_eq!(field_type_to_luau(&arr), "{number}");
    }

    #[test]
    fn test_field_type_to_luau_map() {
        let map = FieldType::Map {
            value: Box::new(FieldType::String),
        };
        assert_eq!(field_type_to_luau(&map), "{ [string]: string }");
    }

    #[test]
    fn test_render_enum_type_basic() {
        let values = vec!["a".to_string(), "b".to_string()];
        assert_eq!(render_enum_type(&values), "(\"a\" | \"b\")");
    }

    #[test]
    fn test_params_with_enum_use_deep_path() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["status"],
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["active", "inactive"]
                }
            }
        });
        let params = json_schema_to_params(&schema);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].luau_type, "(\"active\" | \"inactive\")");
    }
}
