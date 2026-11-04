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
/// Walks `components/schemas`, path-level and operation-level parameter
/// schemas, and requestBody/response content schemas, applying
/// [`normalize_schema_node`] to each. The walk is `$ref`-blind: references
/// resolve later at extraction time, so `$ref` values are left untouched.
pub fn normalize_document(doc: &mut Value) {
    let _ = doc;
    todo!("not implemented")
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
    let _ = node;
    todo!("not implemented")
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
