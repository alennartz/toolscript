//! Dialect normalization for `OpenAPI` schema nodes.
//!
//! `OpenAPI` 3.1/3.2 documents use the JSON Schema 2020-12 data model. Before
//! extraction, schema nodes are normalized into the 3.0 shape the rest of the
//! codegen pipeline understands:
//!
//! - `type: [T, "null"]` becomes `type: T` plus `nullable: true`
//! - `const: v` becomes `enum: [v]`
//! - boolean schemas (`true`/`false`) become `{}` (permissive unknown)
//!
//! 3.0 nodes pass through unchanged. Pure functions, no I/O.

use serde_json::Value;

/// Normalize every schema node in an `OpenAPI` document in place.
///
/// Walks `components/schemas`, component parameters and request bodies,
/// path-level and operation-level parameter schemas, and requestBody/response
/// content schemas, applying
/// [`normalize_schema_node`] to each. The walk is `$ref`-blind: references
/// resolve later at extraction time, so `$ref` values are left untouched.
pub fn normalize_document(doc: &mut Value) {
    if let Some(components) = doc.get_mut("components").and_then(Value::as_object_mut) {
        normalize_components(components);
    }

    let Some(paths) = doc.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    for path_item in paths.values_mut() {
        let Some(path_item) = path_item.as_object_mut() else {
            continue;
        };

        if let Some(parameters) = path_item
            .get_mut("parameters")
            .and_then(Value::as_array_mut)
        {
            for parameter in parameters {
                normalize_parameter(parameter);
            }
        }

        for operation in path_item.values_mut() {
            let Some(operation) = operation.as_object_mut() else {
                continue;
            };

            if let Some(parameters) = operation
                .get_mut("parameters")
                .and_then(Value::as_array_mut)
            {
                for parameter in parameters {
                    normalize_parameter(parameter);
                }
            }

            if let Some(body) = operation.get_mut("requestBody") {
                normalize_content_schemas(body);
            }

            if let Some(responses) = operation
                .get_mut("responses")
                .and_then(Value::as_object_mut)
            {
                for response in responses.values_mut() {
                    normalize_content_schemas(response);
                }
            }
        }
    }
}

/// Normalize the schema-bearing sections of `components`: schemas, parameters,
/// and request bodies.
fn normalize_components(components: &mut serde_json::Map<String, Value>) {
    if let Some(schemas) = components.get_mut("schemas").and_then(Value::as_object_mut) {
        for schema in schemas.values_mut() {
            normalize_schema_node(schema);
        }
    }

    if let Some(parameters) = components
        .get_mut("parameters")
        .and_then(Value::as_object_mut)
    {
        for parameter in parameters.values_mut() {
            normalize_parameter(parameter);
        }
    }

    if let Some(bodies) = components
        .get_mut("requestBodies")
        .and_then(Value::as_object_mut)
    {
        for body in bodies.values_mut() {
            normalize_content_schemas(body);
        }
    }
}

/// Normalize the schema of one parameter object (`#/components/parameters`
/// items, path-level parameters, and operation-level parameters). References
/// pass through untouched; references resolve at extraction time.
fn normalize_parameter(parameter: &mut Value) {
    if let Some(schema) = parameter.get_mut("schema") {
        normalize_schema_node(schema);
    }
}

/// Normalize the schema of every media type in a content map. Covers request
/// bodies and responses; objects without `content` are untouched.
fn normalize_content_schemas(container: &mut Value) {
    let Some(content) = container.get_mut("content").and_then(Value::as_object_mut) else {
        return;
    };
    for media_type in content.values_mut() {
        if let Some(schema) = media_type.get_mut("schema") {
            normalize_schema_node(schema);
        }
    }
}

/// Normalize a single schema node in place.
///
/// - A `type` array with exactly one non-null member becomes that member as
///   `type`, plus `nullable: true` when `"null"` is present
///   (`["string","null"]` -> `{"type":"string","nullable":true}`).
/// - A `type` array that is only `"null"`, or empty, becomes `{}`.
/// - `const: v` becomes `enum: [v]`.
/// - A boolean schema (`true`/`false`) becomes `{}` (permissive unknown).
/// - Already-3.0 nodes pass through unchanged; sibling keys are preserved.
pub fn normalize_schema_node(node: &mut Value) {
    match node {
        // Boolean schemas carry no type information; `{}` keeps the
        // permissive-unknown behavior of the conversion helpers.
        Value::Bool(_) => *node = Value::Object(serde_json::Map::new()),
        Value::Object(fields) => {
            normalize_type_array(fields);
            replace_const_with_enum(fields);
            normalize_schema_children(fields);
        }
        Value::Null | Value::Number(_) | Value::String(_) | Value::Array(_) => {}
    }
}

/// Rewrite a 2020-12 `type` array into the 3.0 shape.
///
/// `[T, "null"]` becomes `type: T` plus `nullable: true`; `"null"`-only or
/// empty arrays drop the node to `{}`; single-member arrays without null keep
/// their member without a `nullable` flag. String `type` values pass through.
fn normalize_type_array(fields: &mut serde_json::Map<String, Value>) {
    let Some(Value::Array(types)) = fields.get("type") else {
        return;
    };

    let type_strings: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
    let non_null: Vec<&str> = type_strings
        .iter()
        .copied()
        .filter(|t| *t != "null")
        .collect();
    let has_null = type_strings.len() != non_null.len();

    if let [single] = non_null.as_slice() {
        fields.insert("type".to_string(), Value::String((*single).to_string()));
        if has_null {
            fields.insert("nullable".to_string(), Value::Bool(true));
        }
    } else {
        // `null`-only, empty, or multi-type arrays (unrepresentable in the
        // 3.0 shape) degrade to the permissive unknown schema.
        fields.remove("type");
        fields.remove("nullable");
    }
}
/// Replace a 2020-12 `const` value with the equivalent single-value `enum`.
fn replace_const_with_enum(fields: &mut serde_json::Map<String, Value>) {
    if let Some(const_value) = fields.remove("const") {
        fields.insert("enum".to_string(), Value::Array(vec![const_value]));
    }
}

/// Recurse into the schema-valued members of an object schema: compositions,
/// array items, property subschemas, and map value schemas. `$ref`-only
/// members stay untouched — references resolve at extraction time.
fn normalize_schema_children(fields: &mut serde_json::Map<String, Value>) {
    for key in ["allOf", "anyOf", "oneOf"] {
        if let Some(members) = fields.get_mut(key).and_then(Value::as_array_mut) {
            for member in members {
                normalize_schema_node(member);
            }
        }
    }

    if let Some(not) = fields.get_mut("not") {
        normalize_schema_node(not);
    }

    if let Some(items) = fields.get_mut("items") {
        normalize_schema_node(items);
    }

    if let Some(properties) = fields.get_mut("properties").and_then(Value::as_object_mut) {
        for property in properties.values_mut() {
            normalize_schema_node(property);
        }
    }

    if let Some(additional) = fields.get_mut("additionalProperties") {
        // `additionalProperties: true/false` is a 3.0 map toggle, not a
        // boolean schema; leave it alone.
        if !additional.is_boolean() {
            normalize_schema_node(additional);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn test_type_array_with_null_becomes_type_plus_nullable() {
        let mut node = json!({"type": ["string", "null"], "description": "A tag"});
        normalize_schema_node(&mut node);
        assert_eq!(node["type"], json!("string"));
        assert_eq!(node["nullable"], json!(true));
    }

    #[test]
    fn test_type_array_null_only_becomes_empty_schema() {
        let mut node = json!({"type": ["null"]});
        normalize_schema_node(&mut node);
        assert_eq!(node, json!({}));
    }

    #[test]
    fn test_type_array_empty_becomes_empty_schema() {
        let mut node = json!({"type": []});
        normalize_schema_node(&mut node);
        assert_eq!(node, json!({}));
    }

    #[test]
    fn test_type_array_without_null_keeps_single_type_and_no_nullable() {
        let mut node = json!({"type": ["integer"]});
        normalize_schema_node(&mut node);
        assert_eq!(node["type"], json!("integer"));
        assert!(node.get("nullable").is_none(), "no nullable key expected");
    }

    #[test]
    fn test_const_becomes_single_value_enum() {
        let mut node = json!({"const": "resource"});
        normalize_schema_node(&mut node);
        assert_eq!(node["enum"], json!(["resource"]));
        assert!(node.get("const").is_none(), "const key should be replaced");
    }

    #[test]
    fn test_boolean_schemas_become_permissive_empty() {
        let mut truthy = json!(true);
        normalize_schema_node(&mut truthy);
        assert_eq!(truthy, json!({}));

        let mut falsy = json!(false);
        normalize_schema_node(&mut falsy);
        assert_eq!(falsy, json!({}));
    }

    #[test]
    fn test_30_style_nodes_pass_through_unchanged() {
        let mut node = json!({
            "type": "string",
            "nullable": true,
            "format": "uuid",
            "enum": ["a", "b"]
        });
        let before = node.clone();
        normalize_schema_node(&mut node);
        assert_eq!(node, before, "3.0-style node must not be rewritten");
    }

    #[test]
    fn test_scalar_type_node_passes_through_unchanged() {
        let mut node = json!({"type": "string", "description": "plain"});
        let before = node.clone();
        normalize_schema_node(&mut node);
        assert_eq!(node, before);
    }

    #[test]
    fn test_sibling_keys_are_preserved_during_normalization() {
        let mut node = json!({
            "type": ["string", "null"],
            "format": "date-time",
            "description": "Last update timestamp"
        });
        normalize_schema_node(&mut node);
        assert_eq!(node["format"], json!("date-time"));
        assert_eq!(node["description"], json!("Last update timestamp"));
        assert_eq!(node["type"], json!("string"));
        assert_eq!(node["nullable"], json!(true));
    }

    #[test]
    fn test_document_normalization_reaches_every_schema_location() {
        let mut doc = json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {
                "/things": {
                    "parameters": [
                        {"name": "limit", "in": "query", "schema": {"type": ["integer", "null"]}}
                    ],
                    "get": {
                        "parameters": [
                            {"name": "q", "in": "query", "schema": {"type": ["string", "null"]}}
                        ],
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": {"type": ["object", "null"]}
                                }
                            }
                        },
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"type": ["array", "null"]}
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "Pet": {
                        "type": "object",
                        "properties": {
                            "tag": {"type": ["string", "null"]}
                        }
                    }
                }
            }
        });

        normalize_document(&mut doc);

        // components/schemas
        assert_eq!(
            doc["components"]["schemas"]["Pet"]["properties"]["tag"]["type"],
            json!("string")
        );
        assert_eq!(
            doc["components"]["schemas"]["Pet"]["properties"]["tag"]["nullable"],
            json!(true)
        );
        // path-level parameter schema
        assert_eq!(
            doc["paths"]["/things"]["parameters"][0]["schema"]["type"],
            json!("integer")
        );
        // operation-level parameter schema
        assert_eq!(
            doc["paths"]["/things"]["get"]["parameters"][0]["schema"]["type"],
            json!("string")
        );
        // requestBody content schema
        assert_eq!(
            doc["paths"]["/things"]["get"]["requestBody"]["content"]["application/json"]["schema"]
                ["type"],
            json!("object")
        );
        // response content schema
        assert_eq!(
            doc["paths"]["/things"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
                ["type"],
            json!("array")
        );
        // non-schema parts are untouched
        assert_eq!(doc["info"]["title"], json!("Test"));
        assert_eq!(doc["openapi"], json!("3.1.0"));
    }

    #[test]
    fn test_document_normalizes_component_parameter_and_body_schemas() {
        let mut doc = json!({
            "openapi": "3.1.0",
            "components": {
                "parameters": {"Limit": {"name": "limit", "in": "query", "schema": {"type": ["integer", "null"]}}},
                "requestBodies": {"Input": {"content": {"application/json": {"schema": {
                    "type": "object", "properties": {"tag": {"type": ["string", "null"]}}
                }}}}}
            }
        });
        normalize_document(&mut doc);
        let parameter = &doc["components"]["parameters"]["Limit"]["schema"];
        assert_eq!(parameter["type"], "integer");
        assert_eq!(parameter["nullable"], true);
        let tag = &doc["components"]["requestBodies"]["Input"]["content"]["application/json"]["schema"]
            ["properties"]["tag"];
        assert_eq!(tag["type"], "string");
        assert_eq!(tag["nullable"], true);
    }

    #[test]
    fn test_document_normalization_leaves_refs_blind() {
        let mut doc = json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {},
            "components": {
                "schemas": {
                    "Holder": {
                        "type": "object",
                        "properties": {
                            "pet": {"$ref": "#/components/schemas/Pet"}
                        }
                    },
                    "Pet": {"type": "object"}
                }
            }
        });

        let reference = doc["components"]["schemas"]["Holder"]["properties"]["pet"]["$ref"].clone();
        normalize_document(&mut doc);
        assert_eq!(
            doc["components"]["schemas"]["Holder"]["properties"]["pet"]["$ref"], reference,
            "$ref values must not be rewritten by normalization"
        );
    }

    #[test]
    fn test_document_normalization_keeps_30_document_untouched() {
        let mut doc = json!({
            "openapi": "3.0.3",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {},
            "components": {
                "schemas": {
                    "Pet": {
                        "type": "object",
                        "properties": {
                            "tag": {"type": "string", "nullable": true}
                        }
                    }
                }
            }
        });

        let before = doc.clone();
        normalize_document(&mut doc);
        assert_eq!(doc, before, "3.0 documents must pass through unchanged");
    }
}
