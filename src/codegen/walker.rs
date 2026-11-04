//! The `OpenAPI` document walker: one extraction path for every supported
//! dialect.
//!
//! Input documents are already validated and normalized (see
//! [`super::normalize`]); the walker is version-agnostic. It walks paths ->
//! operations -> parameters/requestBody/responses/auth/components and produces
//! manifest types via [`super::luau_types`].

use anyhow::Result;
use serde_json::Value;
use url::Url;

use super::manifest::{ApiConfig, AuthConfig, FunctionDef, ParamDef, SchemaDef};

/// Extract the API configuration from a spec document.
///
/// Uses the first `servers[0].url`, resolved against the spec source: a
/// relative server URL joined onto a remote source's origin, a warning plus the
/// raw value for local sources or missing sources.
pub fn extract_api_config(doc: &Value, api_name: &str, spec_source: Option<&str>) -> ApiConfig {
    let _ = (doc, api_name, spec_source);
    todo!("not implemented")
}

/// Extract one [`FunctionDef`] per operation under `paths`.
///
/// Methods: `get`/`post`/`put`/`patch`/`delete`; all others are skipped
/// (including `options`/`head`/`trace` and the 3.2 `query` method).
/// Operation names come from `operationId` (camelCase to `snake_case`) with a
/// method+path fallback when absent.
pub fn extract_functions(doc: &Value, api_name: &str) -> Result<Vec<FunctionDef>> {
    let _ = (doc, api_name);
    todo!("not implemented")
}

/// Extract parameter definitions from a parameter list.
///
/// Cookie parameters are skipped; query/header/path parameters map to
/// [`super::manifest::ParamLocation`]. Type, default, enum, and format come
/// from the schema node. `$ref` parameters resolve against `doc`
/// (`#/components/parameters/{name}`).
pub fn extract_parameters(params: &Value, doc: &Value) -> Result<Vec<ParamDef>> {
    let _ = (params, doc);
    todo!("not implemented")
}

/// Extract schema definitions from `components/schemas`.
///
/// Object schemas become fields; `allOf` schemas merge properties across all
/// sub-schemas with a union of `required` lists. One-level `$ref` members are
/// followed during the merge. Schemas that are neither objects nor `allOf`
/// compositions are skipped.
pub fn extract_schemas(doc: &Value) -> Vec<SchemaDef> {
    let _ = doc;
    todo!("not implemented")
}

/// Extract the authentication configuration from `components/securitySchemes`.
///
/// The first item scheme wins: `http` bearer -> `Bearer`, `http` basic ->
/// `Basic`, `apiKey` (any `in` value) -> `ApiKey` with the scheme name as
/// header. OAuth2/openIdConnect/mutualTLS produce `None`.
pub fn extract_auth_config(doc: &Value) -> Option<AuthConfig> {
    let _ = doc;
    todo!("not implemented")
}

// ---------------------------------------------------------------------------
// Name derivation helpers (ported from parser.rs, behavior unchanged)
// ---------------------------------------------------------------------------

/// Derive a `snake_case` function name from an `operationId` or method+path.
// Unreachable from the stubs until extraction is implemented; part of the
// ported surface this module ships.
#[allow(dead_code)]
pub(crate) fn derive_function_name(operation_id: Option<&str>, method: &str, path: &str) -> String {
    operation_id.map_or_else(|| fallback_function_name(method, path), camel_to_snake)
}

/// Convert `camelCase` to `snake_case`.
///
/// Examples:
///   `listPets` -> `list_pets`
///   `getPetById` -> `get_pet_by_id`
///   `HTMLParser` -> `html_parser`
// Unreachable from the stubs until extraction is implemented; part of the
// ported surface this module ships.
#[allow(dead_code)]
pub(crate) fn camel_to_snake(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::with_capacity(s.len() + 4);

    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                let prev = chars[i - 1];
                let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());

                if prev.is_lowercase() || prev.is_ascii_digit() {
                    // Transition from lowercase/digit to uppercase: always insert underscore
                    // e.g. "listPets" -> underscore before 'P'
                    result.push('_');
                } else if prev.is_uppercase() && next_is_lower {
                    // In an uppercase run, the last uppercase before a lowercase gets an underscore
                    // e.g. "HTMLParser" -> underscore before 'P' (not before H, T, M, L)
                    result.push('_');
                }
            }
            if let Some(lc) = c.to_lowercase().next() {
                result.push(lc);
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Build a function name from method and path when operationId is missing.
///
/// Example: GET `/pets/{petId}` -> `get_pets_by_pet_id`
// Unreachable from the stubs until extraction is implemented; part of the
// ported surface this module ships.
#[allow(dead_code)]
pub(crate) fn fallback_function_name(method: &str, path: &str) -> String {
    let segments: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                // Path parameter: {petId} -> "by_pet_id"
                let param = &seg[1..seg.len() - 1];
                format!("by_{}", camel_to_snake(param))
            } else {
                seg.to_string()
            }
        })
        .collect();

    let mut parts = vec![method.to_lowercase()];
    parts.extend(segments);
    parts.join("_")
}

// ---------------------------------------------------------------------------
// Server URL resolution (ported from parser.rs, behavior unchanged)
// ---------------------------------------------------------------------------

/// Resolve a potentially relative server URL against the spec source.
///
/// If the server URL is already absolute (has a scheme), it is returned as-is.
/// If it is relative (e.g. `/api/v3`) and the spec was fetched from an HTTP(S)
/// URL, the relative path is resolved against that URL's origin.
/// If the spec was loaded from a local file, a relative URL cannot be resolved
/// — a warning is emitted and the raw value is returned unchanged.
// Unreachable from the stubs until extraction is implemented; part of the
// ported surface this module ships.
#[allow(dead_code)]
pub(crate) fn resolve_server_url(server_url: &str, spec_source: Option<&str>) -> String {
    // Already absolute — nothing to do.
    if Url::parse(server_url).is_ok() {
        return server_url.to_string();
    }

    let Some(source) = spec_source else {
        return server_url.to_string();
    };

    // Parse the spec source as a URL. If it isn't one (i.e. a local file path),
    // we can't resolve a relative server URL against it.
    let Ok(source_url) = Url::parse(source) else {
        eprintln!(
            "Warning: server URL {server_url:?} is relative but the spec was loaded from a \
             local path; cannot resolve. Provide an absolute server URL or use a remote spec."
        );
        return server_url.to_string();
    };

    // Build origin: scheme + host + port.
    let origin = source_url.origin();
    match &origin {
        url::Origin::Tuple(scheme, host, port) => {
            let origin_str =
                if (*scheme == "https" && *port == 443) || (*scheme == "http" && *port == 80) {
                    format!("{scheme}://{host}")
                } else {
                    format!("{scheme}://{host}:{port}")
                };
            // Join the relative server path onto the origin.
            let resolved = format!(
                "{}{}",
                origin_str.trim_end_matches('/'),
                if server_url.starts_with('/') {
                    server_url.to_string()
                } else {
                    format!("/{server_url}")
                }
            );
            resolved
        }
        url::Origin::Opaque(_) => {
            eprintln!(
                "Warning: server URL {server_url:?} is relative but spec source {source:?} \
                 has an opaque origin; cannot resolve."
            );
            server_url.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_extract_api_config_reads_servers_info_and_version() {
        let doc = json_doc();
        let config = extract_api_config(&doc, "petstore", None);
        assert_eq!(config.base_url, "https://petstore.example.com/v1");
        assert_eq!(config.version.as_deref(), Some("1.0.0"));
        assert_eq!(config.description.as_deref(), Some("Sample API"));
    }

    #[test]
    fn test_extract_api_config_resolves_relative_server_url() {
        let doc = json_doc();
        let config = extract_api_config(
            &doc,
            "petstore",
            Some("https://petstore3.swagger.io/api/v3/openapi.json"),
        );
        assert_eq!(config.base_url, "https://petstore.example.com/v1");
    }

    #[test]
    fn test_extract_functions_walks_every_method_and_skips_unsupported() {
        let doc = json_doc();
        let functions = extract_functions(&doc, "petstore").unwrap();
        let names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"list_pets"), "got: {names:?}");
        assert!(names.contains(&"create_pet"), "got: {names:?}");
        assert!(names.contains(&"get_pet_by_id"), "got: {names:?}");
        assert!(
            !names
                .iter()
                .any(|n| n.starts_with("head_") || n.starts_with("options_")),
            "options/head/trace must be skipped, got: {names:?}"
        );
    }

    #[test]
    fn test_extract_functions_falls_back_to_method_and_path_naming() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {
                "/pets/{petId}": {
                    "get": {
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        });
        let functions = extract_functions(&doc, "test").unwrap();
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].name, "get_pets_by_pet_id");
        assert_eq!(
            functions[0].method,
            crate::codegen::manifest::HttpMethod::Get
        );
        assert_eq!(functions[0].path, "/pets/{petId}");
    }

    #[test]
    fn test_extract_functions_tolerates_missing_responses_key() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {
                "/ping": {
                    "get": {"operationId": "ping"}
                }
            }
        });
        let functions = extract_functions(&doc, "test").unwrap();
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].name, "ping");
        assert_eq!(functions[0].response_schema, None);
    }

    #[test]
    fn test_extract_parameters_reads_location_type_default_and_enum() {
        let doc = json_doc();
        let params = extract_parameters(&doc["paths"]["/pets"]["get"]["parameters"], &doc).unwrap();
        let status = params.iter().find(|p| p.name == "status").unwrap();
        assert_eq!(
            status.location,
            crate::codegen::manifest::ParamLocation::Query
        );
        assert_eq!(
            status.param_type,
            crate::codegen::manifest::ParamType::String
        );
        assert_eq!(
            status.enum_values,
            Some(vec!["available".to_string(), "pending".to_string()])
        );
        let limit = params.iter().find(|p| p.name == "limit").unwrap();
        assert_eq!(limit.default, Some(serde_json::json!(20)));
    }

    #[test]
    fn test_extract_parameters_skips_cookie_params() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {},
            "components": {
                "parameters": {
                    "Session": {
                        "name": "session",
                        "in": "cookie",
                        "schema": {"type": "string"}
                    },
                    "Limit": {
                        "name": "limit",
                        "in": "query",
                        "schema": {"type": "integer"}
                    }
                }
            }
        });
        let params = extract_parameters(
            &serde_json::json!([
                {"$ref": "#/components/parameters/Session"},
                {"$ref": "#/components/parameters/Limit"}
            ]),
            &doc,
        )
        .unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "limit");
    }

    #[test]
    fn test_extract_parameters_resolves_component_refs() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {},
            "components": {
                "parameters": {
                    "PetId": {
                        "name": "petId",
                        "in": "path",
                        "required": true,
                        "schema": {"type": "string", "format": "uuid"}
                    }
                }
            }
        });
        let params = extract_parameters(
            &serde_json::json!([{"$ref": "#/components/parameters/PetId"}]),
            &doc,
        )
        .unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "petId");
        assert!(params[0].required);
        assert_eq!(params[0].format.as_deref(), Some("uuid"));
    }

    #[test]
    fn test_extract_parameters_rejects_unresolvable_ref() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {}
        });
        let result = extract_parameters(
            &serde_json::json!([{"$ref": "#/components/parameters/Missing"}]),
            &doc,
        );
        assert!(result.is_err(), "unresolvable $ref must error");
    }

    #[test]
    fn test_extract_schemas_reads_object_fields_and_all_of() {
        let doc = json_doc();
        let schemas = extract_schemas(&doc);

        let pet = schemas.iter().find(|s| s.name == "Pet").unwrap();
        let names: Vec<&str> = pet.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"id"), "got: {names:?}");
        assert!(names.contains(&"name"), "got: {names:?}");
        assert!(names.contains(&"status"), "got: {names:?}");

        let tag = pet.fields.iter().find(|f| f.name == "tag").unwrap();
        assert!(
            tag.nullable,
            "normalized type array with null must render nullable"
        );
        assert_eq!(tag.field_type, crate::codegen::manifest::FieldType::String);

        let status = pet.fields.iter().find(|f| f.name == "status").unwrap();
        assert_eq!(
            status.enum_values,
            Some(vec!["available".to_string(), "sold".to_string()])
        );

        let resource = schemas.iter().find(|s| s.name == "Resource").unwrap();
        let names: Vec<&str> = resource.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"id"), "allOf must merge base fields");
        assert!(names.contains(&"name"), "allOf must merge inline fields");
        let id = resource.fields.iter().find(|f| f.name == "id").unwrap();
        assert!(id.required, "allOf must union required lists");
        let name = resource.fields.iter().find(|f| f.name == "name").unwrap();
        assert!(name.required, "allOf must union required lists");
    }

    #[test]
    fn test_extract_schemas_reads_description_and_required_flags() {
        let doc = json_doc();
        let schemas = extract_schemas(&doc);
        let pet = schemas.iter().find(|s| s.name == "Pet").unwrap();
        assert_eq!(pet.description.as_deref(), Some("A pet"));
        let id = pet.fields.iter().find(|f| f.name == "id").unwrap();
        assert!(id.required);
        assert_eq!(id.description.as_deref(), Some("The identifier"));
        let tag = pet.fields.iter().find(|f| f.name == "tag").unwrap();
        assert!(!tag.required, "tag is not in the required list");
    }

    #[test]
    fn test_extract_schemas_reads_map_via_additional_properties() {
        let doc = json_doc();
        let schemas = extract_schemas(&doc);
        let resource = schemas.iter().find(|s| s.name == "Resource").unwrap();
        let metadata = resource
            .fields
            .iter()
            .find(|f| f.name == "metadata")
            .unwrap();
        assert_eq!(
            metadata.field_type,
            crate::codegen::manifest::FieldType::Map {
                value: Box::new(crate::codegen::manifest::FieldType::String)
            }
        );
    }

    #[test]
    fn test_extract_schemas_skips_non_object_schemas() {
        let doc = json_doc();
        let schemas = extract_schemas(&doc);
        assert!(
            !schemas.iter().any(|s| s.name == "Status"),
            "non-object schemas must be skipped, got: {:?}",
            schemas.iter().map(|s| s.name.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_extract_schemas_returns_empty_without_components() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {}
        });
        assert!(extract_schemas(&doc).is_empty());
    }

    #[test]
    fn test_extract_auth_config_reads_bearer_basic_and_api_key() {
        let doc = json_doc();
        let auth = extract_auth_config(&doc).unwrap();
        assert_eq!(
            auth,
            crate::codegen::manifest::AuthConfig::Bearer {
                header: "Authorization".to_string(),
                prefix: "Bearer ".to_string()
            }
        );
    }

    #[test]
    fn test_extract_auth_config_picks_first_item_scheme() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {},
            "components": {
                "securitySchemes": {
                    "key": {"type": "apiKey", "name": "X-Api-Key", "in": "header"},
                    "unused": {"type": "http", "scheme": "basic"}
                }
            }
        });
        let auth = extract_auth_config(&doc).unwrap();
        assert_eq!(
            auth,
            crate::codegen::manifest::AuthConfig::ApiKey {
                header: "X-Api-Key".to_string()
            }
        );
    }

    #[test]
    fn test_extract_auth_config_skips_oauth2_and_mutual_tls() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {},
            "components": {
                "securitySchemes": {
                    "oauth": {"type": "oauth2", "flows": {}},
                    "mtls": {"type": "mutualTLS"}
                }
            }
        });
        assert!(extract_auth_config(&doc).is_none());
    }

    #[test]
    fn test_extract_auth_config_ignores_reference_schemes() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {},
            "components": {
                "securitySchemes": {
                    "ref": {"$ref": "#/components/securitySchemes/real"}
                }
            }
        });
        assert!(extract_auth_config(&doc).is_none());
    }

    #[test]
    fn test_extract_functions_reads_request_body_content_type_and_required() {
        let doc = json_doc();
        let functions = extract_functions(&doc, "petstore").unwrap();
        let create = functions.iter().find(|f| f.name == "create_pet").unwrap();
        let body = create.request_body.as_ref().unwrap();
        assert_eq!(body.content_type, "application/json");
        assert_eq!(body.schema, "NewPet");
        assert!(body.required);
    }

    #[test]
    fn test_extract_functions_selects_first_2xx_json_response() {
        let doc = json_doc();
        let functions = extract_functions(&doc, "petstore").unwrap();
        let get_pet = functions
            .iter()
            .find(|f| f.name == "get_pet_by_id")
            .unwrap();
        assert_eq!(get_pet.response_schema.as_deref(), Some("Pet"));
    }

    #[test]
    fn test_extract_functions_unwraps_array_item_response_schema() {
        let doc = json_doc();
        let functions = extract_functions(&doc, "petstore").unwrap();
        let list = functions.iter().find(|f| f.name == "list_pets").unwrap();
        assert_eq!(list.response_schema.as_deref(), Some("Pet"));
    }

    #[test]
    fn test_resolve_absolute_url_unchanged() {
        let result = resolve_server_url(
            "https://petstore.example.com/v1",
            Some("https://example.com/openapi.json"),
        );
        assert_eq!(result, "https://petstore.example.com/v1");
    }

    #[test]
    fn test_resolve_relative_url_against_http_source() {
        let result = resolve_server_url(
            "/api/v3",
            Some("https://petstore3.swagger.io/api/v3/openapi.json"),
        );
        assert_eq!(result, "https://petstore3.swagger.io/api/v3");
    }

    #[test]
    fn test_resolve_relative_root_against_http_source() {
        let result = resolve_server_url("/", Some("https://api.example.com/specs/openapi.json"));
        assert_eq!(result, "https://api.example.com/");
    }

    #[test]
    fn test_resolve_relative_url_preserves_port() {
        let result = resolve_server_url(
            "/api/v1",
            Some("https://api.example.com:8443/specs/openapi.json"),
        );
        assert_eq!(result, "https://api.example.com:8443/api/v1");
    }

    #[test]
    fn test_resolve_relative_url_default_port_omitted() {
        let result = resolve_server_url(
            "/api/v1",
            Some("https://api.example.com:443/specs/openapi.json"),
        );
        assert_eq!(result, "https://api.example.com/api/v1");
    }

    #[test]
    fn test_resolve_relative_url_http_default_port_omitted() {
        let result = resolve_server_url(
            "/api/v1",
            Some("http://api.example.com:80/specs/openapi.json"),
        );
        assert_eq!(result, "http://api.example.com/api/v1");
    }

    #[test]
    fn test_resolve_relative_url_local_file_unchanged() {
        let result = resolve_server_url("/api/v3", Some("testdata/petstore.yaml"));
        assert_eq!(result, "/api/v3");
    }

    #[test]
    fn test_resolve_relative_url_no_source() {
        let result = resolve_server_url("/api/v3", None);
        assert_eq!(result, "/api/v3");
    }

    #[test]
    fn test_extract_functions_sets_tags_summary_and_deprecation() {
        let doc = json_doc();
        let functions = extract_functions(&doc, "petstore").unwrap();
        let list = functions.iter().find(|f| f.name == "list_pets").unwrap();
        assert_eq!(list.tag.as_deref(), Some("pets"));
        assert_eq!(list.summary.as_deref(), Some("List pets"));
        assert!(!list.deprecated);
        let legacy = functions.iter().find(|f| f.name == "legacy_op").unwrap();
        assert!(legacy.deprecated);
    }

    #[test]
    fn test_extract_functions_request_body_falls_back_to_first_content_type() {
        let doc = json_doc();
        let functions = extract_functions(&doc, "petstore").unwrap();
        let upload = functions.iter().find(|f| f.name == "upload_logo").unwrap();
        let body = upload.request_body.as_ref().unwrap();
        assert_eq!(body.content_type, "text/plain");
        assert_eq!(body.schema, "unknown");
    }

    #[test]
    fn test_extract_functions_tolerates_missing_operation_fields() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Test", "version": "1.0.0"},
            "paths": {
                "/things": {
                    "post": {"operationId": "postThings"}
                }
            }
        });
        let functions = extract_functions(&doc, "test").unwrap();
        assert_eq!(functions.len(), 1);
        assert!(functions[0].parameters.is_empty());
        assert!(functions[0].request_body.is_none());
    }

    #[test]
    fn test_extract_api_config_resolves_relative_server_url_against_http_source() {
        let doc = json_doc();
        let config = extract_api_config(
            &doc,
            "petstore",
            Some("https://petstore3.swagger.io/api/v3/openapi.json"),
        );
        assert_eq!(config.base_url, "https://petstore3.swagger.io/api/v3");
    }

    /// Fixture document shared by the walker tests: 3.1-style constructs
    /// already normalized (or normalization-neutral).
    #[allow(clippy::too_many_lines)]
    fn json_doc() -> serde_json::Value {
        serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Petstore", "version": "1.0.0", "description": "Sample API"},
            "servers": [{"url": "https://petstore.example.com/v1"}],
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "listPets",
                        "summary": "List pets",
                        "parameters": [
                            {
                                "name": "status",
                                "in": "query",
                                "schema": {"type": "string", "enum": ["available", "pending", "sold"]}
                            },
                            {
                                "name": "limit",
                                "in": "query",
                                "schema": {"type": "integer", "default": 20}
                            }
                        ],
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "array",
                                            "items": {"$ref": "#/components/schemas/Pet"}
                                        }
                                    }
                                }
                            }
                        }
                    },
                    "post": {
                        "tags": ["pets"],
                        "operationId": "createPet",
                        "requestBody": {
                            "required": true,
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/NewPet"}
                                }
                            }
                        },
                        "responses": {
                            "201": {
                                "description": "created",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/Pet"}
                                    }
                                }
                            }
                        }
                    }
                },
                "/pets/{petId}": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "getPetById",
                        "parameters": [
                            {
                                "name": "petId",
                                "in": "path",
                                "required": true,
                                "schema": {"type": "string"}
                            }
                        ],
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/Pet"}
                                    }
                                }
                            },
                            "404": {"description": "not found"}
                        }
                    }
                },
                "/logo": {
                    "put": {
                        "operationId": "uploadLogo",
                        "requestBody": {
                            "content": {
                                "text/plain": {"schema": {"type": "string"}}
                            }
                        },
                        "responses": {"204": {"description": "done"}}
                    }
                },
                "/legacy": {
                    "delete": {
                        "operationId": "legacyOp",
                        "deprecated": true,
                        "responses": {"204": {"description": "gone"}}
                    }
                },
                "/meta": {
                    "head": {"operationId": "headMeta", "responses": {"200": {"description": "ok"}}}
                }
            },
            "components": {
                "schemas": {
                    "Pet": {
                        "type": "object",
                        "description": "A pet",
                        "required": ["id", "name", "status"],
                        "properties": {
                            "id": {"type": "string", "description": "The identifier"},
                            "name": {"type": "string"},
                            "status": {"type": "string", "enum": ["available", "sold"]},
                            "tag": {"type": ["string", "null"]}
                        }
                    },
                    "NewPet": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {"name": {"type": "string"}}
                    },
                    "Resource": {
                        "allOf": [
                            {"$ref": "#/components/schemas/Base"},
                            {
                                "type": "object",
                                "required": ["name"],
                                "properties": {
                                    "name": {"type": "string"},
                                    "metadata": {
                                        "type": "object",
                                        "additionalProperties": {"type": "string"}
                                    }
                                }
                            }
                        ]
                    },
                    "Base": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {"id": {"type": "string"}}
                    },
                    "Status": {"type": "string", "enum": ["on", "off"]}
                }
            }
        })
    }
}
