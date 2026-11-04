//! The `OpenAPI` document walker: one extraction path for every supported
//! dialect.
//!
//! Input documents are already validated and normalized (see
//! [`super::normalize`]); the walker is version-agnostic. It walks paths ->
//! operations -> parameters/requestBody/responses/auth/components and produces
//! manifest types via [`super::luau_types`].

use anyhow::{Context, Result};
use serde_json::Value;
use url::Url;

use super::luau_types;
use super::manifest::{
    ApiConfig, AuthConfig, FieldDef, FieldType, FunctionDef, HttpMethod, ParamDef, ParamLocation,
    ParamType, RequestBodyDef, SchemaDef,
};

/// Supported operation methods in extraction order (ported method order).
const SUPPORTED_METHODS: [(&str, HttpMethod); 5] = [
    ("get", HttpMethod::Get),
    ("put", HttpMethod::Put),
    ("post", HttpMethod::Post),
    ("delete", HttpMethod::Delete),
    ("patch", HttpMethod::Patch),
];

/// Extract the API configuration from a spec document.
///
/// Uses the first `servers[0].url`, resolved against the spec source: a
/// relative server URL joined onto a remote source's origin, a warning plus the
/// raw value for local sources or missing sources.
pub fn extract_api_config(doc: &Value, api_name: &str, spec_source: Option<&str>) -> ApiConfig {
    let raw_url = doc
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|servers| servers.first())
        .and_then(|server| server.get("url"))
        .and_then(Value::as_str)
        .map_or_else(|| "/".to_string(), String::from);

    ApiConfig {
        name: api_name.to_string(),
        base_url: resolve_server_url(&raw_url, spec_source),
        description: info_field(doc, "description"),
        version: info_field(doc, "version"),
        auth: extract_auth_config(doc),
    }
}

/// Read a string field from the document's `info` object.
fn info_field(doc: &Value, key: &str) -> Option<String> {
    doc.get("info").and_then(|info| string_field(info, key))
}

/// Extract one [`FunctionDef`] per operation under `paths`.
///
/// Methods: `get`/`post`/`put`/`patch`/`delete`; all others are skipped
/// (including `options`/`head`/`trace` and the 3.2 `query` method).
/// Operation names come from `operationId` (camelCase to `snake_case`) with a
/// method+path fallback when absent.
pub fn extract_functions(doc: &Value, api_name: &str) -> Result<Vec<FunctionDef>> {
    let mut functions = Vec::new();
    let Some(paths) = doc.get("paths").and_then(Value::as_object) else {
        return Ok(functions);
    };

    for (path, item) in paths {
        // Referenced path items are skipped, as before.
        if item.get("$ref").is_some() {
            continue;
        }
        for (method, http_method) in SUPPORTED_METHODS {
            let Some(operation) = item.get(method) else {
                continue;
            };

            let parameters =
                extract_parameters(operation.get("parameters").unwrap_or(&Value::Null), doc)?;
            let request_body = extract_request_body(operation.get("requestBody"), doc)?;

            functions.push(FunctionDef {
                name: derive_function_name(
                    operation.get("operationId").and_then(Value::as_str),
                    method,
                    path,
                ),
                api: api_name.to_string(),
                tag: operation
                    .get("tags")
                    .and_then(Value::as_array)
                    .and_then(|tags| tags.first())
                    .and_then(Value::as_str)
                    .map(String::from),
                method: http_method,
                path: path.clone(),
                summary: string_field(operation, "summary"),
                description: string_field(operation, "description"),
                deprecated: bool_field(operation, "deprecated"),
                parameters,
                request_body,
                response_schema: extract_response_schema(operation.get("responses")),
            });
        }
    }

    Ok(functions)
}

/// Extract parameter definitions from a parameter list.
///
/// Cookie parameters are skipped; query/header/path parameters map to
/// [`super::manifest::ParamLocation`]. Type, default, enum, and format come
/// from the schema node. `$ref` parameters resolve against `doc`
/// (`#/components/parameters/{name}`).
pub fn extract_parameters(params: &Value, doc: &Value) -> Result<Vec<ParamDef>> {
    let mut result = Vec::new();
    let Some(entries) = params.as_array() else {
        return Ok(result);
    };

    for entry in entries {
        let param = resolve_parameter(entry, doc)?;
        let location = match param.get("in").and_then(Value::as_str) {
            Some("query") => ParamLocation::Query,
            Some("header") => ParamLocation::Header,
            Some("path") => ParamLocation::Path,
            _ => continue, // Cookie params are skipped.
        };

        let name = param
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let schema = param.get("schema").unwrap_or(&Value::Null);
        let field = luau_types::json_schema_prop_to_field_def(name, schema, false);

        result.push(ParamDef {
            name: field.name,
            location,
            param_type: param_type_from_field(&field.field_type),
            required: bool_field(param, "required"),
            description: string_field(param, "description"),
            default: schema.get("default").cloned(),
            enum_values: field.enum_values,
            format: field.format,
            frozen_value: None,
        });
    }

    Ok(result)
}

/// Resolve one parameter entry, following `#/components/parameters/{name}`.
fn resolve_parameter<'a>(entry: &'a Value, doc: &'a Value) -> Result<&'a Value> {
    match entry.get("$ref").and_then(Value::as_str) {
        Some(reference) => {
            let name = reference
                .strip_prefix("#/components/parameters/")
                .with_context(|| format!("Unsupported parameter $ref: {reference}"))?;
            let components = doc
                .get("components")
                .context("Spec has $ref but no components")?;
            components
                .get("parameters")
                .and_then(|parameters| parameters.get(name))
                .with_context(|| format!("Could not resolve parameter ref: {reference}"))
        }
        None => Ok(entry),
    }
}

/// Map a converted field type onto the parameter type vocabulary.
///
/// Compound types fall back to `String`, matching the previous default.
const fn param_type_from_field(field_type: &FieldType) -> ParamType {
    match field_type {
        FieldType::Integer => ParamType::Integer,
        FieldType::Number => ParamType::Number,
        FieldType::Boolean => ParamType::Boolean,
        _ => ParamType::String,
    }
}

/// Extract schema definitions from `components/schemas`.
///
/// Object schemas become fields; `allOf` schemas merge properties across all
/// sub-schemas with a union of `required` lists. One-level `$ref` members are
/// followed during the merge. Schemas that are neither objects nor `allOf`
/// compositions are skipped.
pub fn extract_schemas(doc: &Value) -> Vec<SchemaDef> {
    let Some(schemas) = doc
        .get("components")
        .and_then(|components| components.get("schemas"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };

    schemas
        .iter()
        .filter_map(|(name, schema)| extract_schema_def(name, schema, doc))
        .collect()
}

/// Extract the authentication configuration from `components/securitySchemes`.
///
/// The first supported inline scheme wins: `http` bearer -> `Bearer`, `http`
/// basic -> `Basic`, `apiKey` (any `in` value) -> `ApiKey` with the scheme
/// name as header. Unsupported schemes and references are skipped.
pub fn extract_auth_config(doc: &Value) -> Option<AuthConfig> {
    let schemes = doc.get("components")?.get("securitySchemes")?.as_object()?;

    schemes.values().find_map(supported_auth_scheme)
}

/// Map one security scheme object onto an [`AuthConfig`], or `None` when the
/// scheme is a reference or unsupported.
fn supported_auth_scheme(scheme: &Value) -> Option<AuthConfig> {
    if scheme.get("$ref").is_some() {
        return None;
    }
    match scheme.get("type").and_then(Value::as_str) {
        Some("http") => match string_field(scheme, "scheme").as_deref() {
            Some(s) if s.eq_ignore_ascii_case("bearer") => Some(AuthConfig::Bearer {
                header: "Authorization".to_string(),
                prefix: "Bearer ".to_string(),
            }),
            Some(s) if s.eq_ignore_ascii_case("basic") => Some(AuthConfig::Basic),
            _ => None,
        },
        Some("apiKey") => string_field(scheme, "name").map(|header| AuthConfig::ApiKey { header }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Request body and response extraction
// ---------------------------------------------------------------------------

/// Extract a request body definition from an operation's `requestBody`.
fn extract_request_body(body: Option<&Value>, doc: &Value) -> Result<Option<RequestBodyDef>> {
    let Some(body) = body else {
        return Ok(None);
    };
    let resolved = resolve_request_body(body, doc)?;
    let Some(content) = resolved.get("content").and_then(Value::as_object) else {
        return Ok(None);
    };

    // Prefer application/json, then the first content type in document order.
    let selected = content
        .iter()
        .find(|(content_type, _)| content_type.contains("json"))
        .or_else(|| content.iter().next());
    let Some((content_type, media_type)) = selected else {
        return Ok(None);
    };

    Ok(Some(RequestBodyDef {
        content_type: content_type.clone(),
        schema: media_type
            .get("schema")
            .and_then(ref_schema_name)
            .unwrap_or_else(|| "unknown".to_string()),
        required: bool_field(resolved, "required"),
        description: string_field(resolved, "description"),
    }))
}

/// Resolve a request body, following `#/components/requestBodies/{name}`.
fn resolve_request_body<'a>(body: &'a Value, doc: &'a Value) -> Result<&'a Value> {
    match body.get("$ref").and_then(Value::as_str) {
        Some(reference) => {
            let name = reference
                .strip_prefix("#/components/requestBodies/")
                .with_context(|| format!("Unsupported requestBody $ref: {reference}"))?;
            let components = doc
                .get("components")
                .context("Spec has $ref but no components")?;
            components
                .get("requestBodies")
                .and_then(|bodies| bodies.get(name))
                .with_context(|| format!("Could not resolve requestBody ref: {reference}"))
        }
        None => Ok(body),
    }
}

/// Extract the component schema name from a schema node reference.
fn ref_schema_name(schema: &Value) -> Option<String> {
    schema
        .get("$ref")
        .and_then(Value::as_str)?
        .strip_prefix("#/components/schemas/")
        .map(String::from)
}

/// Select the response schema name for an operation.
///
/// The first numeric 2xx response with an `application/json` schema name
/// wins; array item references unwrap; the `default` response is the
/// fallback. Wildcard `2XX` ranges are ignored.
fn extract_response_schema(responses: Option<&Value>) -> Option<String> {
    let statuses = responses.and_then(Value::as_object)?;
    for code in 200..=299u16 {
        let code = code.to_string();
        if let Some(response) = statuses.get(code.as_str())
            && let Some(name) = response_schema_name(response)
        {
            return Some(name);
        }
    }
    response_schema_name(statuses.get("default")?)
}

/// Read the schema name from one response's `application/json` content.
fn response_schema_name(response: &Value) -> Option<String> {
    let schema = response
        .get("content")?
        .get("application/json")?
        .get("schema")?;
    if let Some(name) = ref_schema_name(schema) {
        return Some(name);
    }
    ref_schema_name(schema.get("items")?)
}

// ---------------------------------------------------------------------------
// Schema extraction
// ---------------------------------------------------------------------------

/// Extract one schema definition, or `None` when the schema is neither an
/// object nor an `allOf` composition.
fn extract_schema_def(name: &str, schema: &Value, doc: &Value) -> Option<SchemaDef> {
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let required = required_set(schema);
        let fields = properties
            .iter()
            .map(|(field_name, field_schema)| {
                extract_field_def(
                    field_name,
                    field_schema,
                    required.contains(field_name.as_str()),
                    doc,
                )
            })
            .collect();
        return Some(SchemaDef {
            name: name.to_string(),
            description: string_field(schema, "description"),
            fields,
        });
    }

    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        let mut properties: Vec<(String, Value)> = Vec::new();
        let mut required: Vec<String> = Vec::new();
        for member in all_of {
            collect_object_properties(member, doc, &mut properties, &mut required);
        }
        let required = required_set_from(&required);
        let fields = properties
            .iter()
            .map(|(field_name, field_schema)| {
                extract_field_def(
                    field_name,
                    field_schema,
                    required.contains(field_name.as_str()),
                    doc,
                )
            })
            .collect();
        return Some(SchemaDef {
            name: name.to_string(),
            description: string_field(schema, "description"),
            fields,
        });
    }

    None
}

/// Recursively collect properties and required fields from an `allOf` member,
/// following one level of `$ref` into `components/schemas` per member and
/// recursing through nested `allOf` members. Property order is preserved.
fn collect_object_properties(
    schema: &Value,
    doc: &Value,
    properties: &mut Vec<(String, Value)>,
    required: &mut Vec<String>,
) {
    let schema = match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => {
            let Some(name) = reference.strip_prefix("#/components/schemas/") else {
                return;
            };
            let Some(resolved) = doc
                .get("components")
                .and_then(|components| components.get("schemas"))
                .and_then(|schemas| schemas.get(name))
            else {
                return;
            };
            resolved
        }
        None => schema,
    };

    if let Some(member_properties) = schema.get("properties").and_then(Value::as_object) {
        properties.extend(
            member_properties
                .iter()
                .map(|(field_name, field_schema)| (field_name.clone(), field_schema.clone())),
        );
    }
    if let Some(member_required) = schema.get("required").and_then(Value::as_array) {
        required.extend(
            member_required
                .iter()
                .filter_map(Value::as_str)
                .map(String::from),
        );
    }
    if let Some(nested) = schema.get("allOf").and_then(Value::as_array) {
        for member in nested {
            collect_object_properties(member, doc, properties, required);
        }
    }
}

/// Extract one field definition. Referenced fields become
/// [`FieldType::Object`] with the referenced schema's description; inline
/// schemas convert through [`luau_types::json_schema_prop_to_field_def`].
fn extract_field_def(name: &str, schema: &Value, required: bool, doc: &Value) -> FieldDef {
    if let Some(schema_name) = ref_schema_name(schema) {
        let description = doc
            .get("components")
            .and_then(|components| components.get("schemas"))
            .and_then(|schemas| schemas.get(&schema_name))
            .and_then(|referenced| string_field(referenced, "description"));
        return FieldDef {
            name: name.to_string(),
            field_type: FieldType::Object {
                schema: schema_name,
            },
            required,
            description,
            enum_values: None,
            nullable: false,
            format: None,
        };
    }
    luau_types::json_schema_prop_to_field_def(name, schema, required)
}

/// Collect a schema's `required` list as a string set.
fn required_set(schema: &Value) -> std::collections::HashSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Collect an accumulated `required` name list as a string set.
fn required_set_from(required: &[String]) -> std::collections::HashSet<String> {
    required.iter().cloned().collect()
}

// ---------------------------------------------------------------------------
// Small field readers
// ---------------------------------------------------------------------------

/// Read a string field from a JSON object.
fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(String::from)
}

/// Read a boolean field from a JSON object, defaulting to `false`.
fn bool_field(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Name derivation helpers (ported from parser.rs, behavior unchanged)
// ---------------------------------------------------------------------------

/// Derive a `snake_case` function name from an `operationId` or method+path.
pub(crate) fn derive_function_name(operation_id: Option<&str>, method: &str, path: &str) -> String {
    operation_id.map_or_else(|| fallback_function_name(method, path), camel_to_snake)
}

/// Convert `camelCase` to `snake_case`.
///
/// Examples:
///   `listPets` -> `list_pets`
///   `getPetById` -> `get_pet_by_id`
///   `HTMLParser` -> `html_parser`
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
            Some(vec![
                "available".to_string(),
                "pending".to_string(),
                "sold".to_string()
            ])
        );
        let limit = params.iter().find(|p| p.name == "limit").unwrap();
        assert_eq!(limit.default, Some(serde_json::json!(20)));
    }

    #[test]
    fn test_extract_parameters_maps_header_location() {
        let params = extract_parameters(
            &serde_json::json!([
                {"name": "X-Request-Id", "in": "header", "schema": {"type": "string"}},
                {"name": "q", "in": "query", "schema": {"type": "boolean"}},
                {"name": "rate", "in": "query", "schema": {"type": "number"}}
            ]),
            &serde_json::json!({}),
        )
        .unwrap();
        let header = params.iter().find(|p| p.name == "X-Request-Id").unwrap();
        assert_eq!(
            header.location,
            crate::codegen::manifest::ParamLocation::Header
        );
        let boolean = params.iter().find(|p| p.name == "q").unwrap();
        assert_eq!(
            boolean.param_type,
            crate::codegen::manifest::ParamType::Boolean
        );
        let number = params.iter().find(|p| p.name == "rate").unwrap();
        assert_eq!(
            number.param_type,
            crate::codegen::manifest::ParamType::Number
        );
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
    fn test_extract_schemas_reference_field_uses_referenced_description() {
        let doc = serde_json::json!({
            "components": {
                "schemas": {
                    "Holder": {
                        "type": "object",
                        "properties": {"pet": {"$ref": "#/components/schemas/Pet"}}
                    },
                    "Pet": {"type": "object", "description": "A pet"}
                }
            }
        });
        let schemas = extract_schemas(&doc);
        let holder = schemas.iter().find(|s| s.name == "Holder").unwrap();
        let pet = holder.fields.iter().find(|f| f.name == "pet").unwrap();
        assert_eq!(
            pet.field_type,
            crate::codegen::manifest::FieldType::Object {
                schema: "Pet".to_string()
            }
        );
        assert_eq!(pet.description.as_deref(), Some("A pet"));
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
    fn test_extract_auth_config_maps_http_basic() {
        let doc = serde_json::json!({
            "components": {
                "securitySchemes": {
                    "creds": {"type": "http", "scheme": "Basic"}
                }
            }
        });
        assert_eq!(
            extract_auth_config(&doc),
            Some(crate::codegen::manifest::AuthConfig::Basic)
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
    fn test_api_config_resolves_server_urls() {
        let cases = [
            (
                "https://petstore.example.com/v1",
                Some("https://example.com/openapi.json"),
                "https://petstore.example.com/v1",
            ),
            (
                "/api/v3",
                Some("https://petstore3.swagger.io/api/v3/openapi.json"),
                "https://petstore3.swagger.io/api/v3",
            ),
            (
                "/",
                Some("https://api.example.com/specs/openapi.json"),
                "https://api.example.com/",
            ),
            (
                "/api/v1",
                Some("https://api.example.com:8443/specs/openapi.json"),
                "https://api.example.com:8443/api/v1",
            ),
            (
                "/api/v1",
                Some("https://api.example.com:443/specs/openapi.json"),
                "https://api.example.com/api/v1",
            ),
            (
                "/api/v1",
                Some("http://api.example.com:80/specs/openapi.json"),
                "http://api.example.com/api/v1",
            ),
            ("/api/v3", Some("testdata/petstore.yaml"), "/api/v3"),
            ("/api/v3", None, "/api/v3"),
        ];
        for (url, source, expected) in cases {
            let doc = serde_json::json!({"servers": [{"url": url}]});
            assert_eq!(extract_api_config(&doc, "test", source).base_url, expected);
        }
    }

    #[test]
    fn test_operation_names() {
        let cases = [
            ("get", "/pets", Some("listPets"), "list_pets"),
            ("get", "/pets", Some("getPetById"), "get_pet_by_id"),
            ("post", "/pets", Some("createPet"), "create_pet"),
            ("get", "/pets", Some("list_pets"), "list_pets"),
            ("get", "/pets", Some("HTMLParser"), "html_parser"),
            ("get", "/pets", Some("getAPIKey"), "get_api_key"),
            ("get", "/pets", Some("pets"), "pets"),
            ("get", "/pets", Some("Pets"), "pets"),
            ("get", "/pets", None, "get_pets"),
            ("get", "/pets/{petId}", None, "get_pets_by_pet_id"),
            ("post", "/pets", None, "post_pets"),
            (
                "delete",
                "/users/{userId}/posts/{postId}",
                None,
                "delete_users_by_user_id_posts_by_post_id",
            ),
        ];
        for (method, path, id, expected) in cases {
            let mut op = serde_json::json!({"responses": {}});
            if let Some(id) = id {
                op["operationId"] = serde_json::json!(id);
            }
            let mut doc = serde_json::json!({"paths": {}});
            doc["paths"][path] = serde_json::json!({(method): op});
            let functions = extract_functions(&doc, "test").unwrap();
            assert_eq!(functions[0].name, expected);
        }
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
        let mut doc = json_doc();
        doc["servers"][0]["url"] = serde_json::json!("/api/v3");
        let config = extract_api_config(
            &doc,
            "petstore",
            Some("https://petstore3.swagger.io/api/v3/openapi.json"),
        );
        assert_eq!(config.base_url, "https://petstore3.swagger.io/api/v3");
    }

    #[test]
    fn test_skips_unsupported_methods() {
        let doc = serde_json::json!({"paths": {"/items": {
            "get": {"operationId": "getItems"},
            "head": {"operationId": "headItems"},
            "options": {"operationId": "optionsItems"},
            "trace": {"operationId": "traceItems"},
            "query": {"operationId": "queryItems"}
        }}});
        let functions = extract_functions(&doc, "test").unwrap();
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].name, "get_items");
    }

    #[test]
    fn test_body_reference_and_json_content_preference() {
        let doc = serde_json::json!({
            "paths": {"/items": {"post": {"operationId": "create", "requestBody": {"$ref": "#/components/requestBodies/Item"}}}},
            "components": {"requestBodies": {"Item": {
                "required": true, "description": "An item", "content": {
                    "text/plain": {"schema": {"type": "string"}},
                    "application/json": {"schema": {"$ref": "#/components/schemas/Item"}}
                }
            }}}
        });
        let functions = extract_functions(&doc, "test").unwrap();
        let body = functions[0].request_body.as_ref().unwrap();
        assert_eq!(body.content_type, "application/json");
        assert_eq!(body.schema, "Item");
        assert!(body.required);
        assert_eq!(body.description.as_deref(), Some("An item"));

        let missing = serde_json::json!({"paths": {"/items": {"post": {
            "requestBody": {"$ref": "#/components/requestBodies/Missing"}
        }}}});
        assert!(extract_functions(&missing, "test").is_err());
    }

    #[test]
    fn test_default_response_fallback_and_wildcard_ignored() {
        let doc = serde_json::json!({"paths": {"/items": {"get": {
            "responses": {
                "2XX": {"description": "wildcard", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Wildcard"}}}},
                "default": {"description": "default", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Fallback"}}}}
            }
        }}}});
        let functions = extract_functions(&doc, "test").unwrap();
        assert_eq!(functions[0].response_schema.as_deref(), Some("Fallback"));
    }

    #[test]
    fn test_auth_skips_unsupported_and_refs_before_first_supported_scheme() {
        let doc = serde_json::json!({"components": {"securitySchemes": {
            "oauth": {"type": "oauth2", "flows": {}},
            "openid": {"type": "openIdConnect", "openIdConnectUrl": "https://example.com"},
            "mtls": {"type": "mutualTLS"},
            "reference": {"$ref": "#/components/securitySchemes/key"},
            "key": {"type": "apiKey", "name": "X-Key", "in": "header"},
            "basic": {"type": "http", "scheme": "basic"}
        }}});
        assert_eq!(
            extract_auth_config(&doc),
            Some(crate::codegen::manifest::AuthConfig::ApiKey {
                header: "X-Key".into()
            })
        );
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
                            "tag": {"type": "string", "nullable": true}
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
                },
                "securitySchemes": {
                    "bearer": {"type": "http", "scheme": "bearer"}
                }
            }
        })
    }
}
