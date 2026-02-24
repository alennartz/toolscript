use std::path::Path;

use anyhow::{Context, Result};
use openapiv3::{
    OpenAPI, Parameter, ParameterSchemaOrContent, ReferenceOr, Schema, SchemaKind, SecurityScheme,
    Type,
};

use super::manifest::{
    ApiConfig, AuthConfig, FieldDef, FieldType, FunctionDef, HttpMethod, Manifest, ParamDef,
    ParamLocation, ParamType, RequestBodyDef, SchemaDef,
};

/// Load an `OpenAPI` spec from a local YAML or JSON file.
pub fn load_spec_from_file(path: &Path) -> Result<OpenAPI> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    // Try YAML first (which is a superset of JSON), then fall back to JSON
    let spec: OpenAPI = serde_yaml::from_str(&content)
        .or_else(|_| serde_json::from_str(&content))
        .with_context(|| format!("Failed to parse OpenAPI spec from {}", path.display()))?;

    Ok(spec)
}

/// Fetch and parse an `OpenAPI` spec from a URL.
pub async fn load_spec_from_url(url: &str) -> Result<OpenAPI> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("Failed to fetch spec from {url}"))?;

    let content = response
        .text()
        .await
        .with_context(|| format!("Failed to read response body from {url}"))?;

    let spec: OpenAPI = serde_yaml::from_str(&content)
        .or_else(|_| serde_json::from_str(&content))
        .with_context(|| format!("Failed to parse OpenAPI spec from {url}"))?;

    Ok(spec)
}

/// Convert an `OpenAPI` spec into a `Manifest`.
///
/// Walks the spec and extracts:
/// 1. `ApiConfig` from info + servers + security schemes
/// 2. `FunctionDef` from each path + operation
/// 3. `SchemaDef` from components/schemas
pub fn spec_to_manifest(spec: &OpenAPI, api_name: &str) -> Result<Manifest> {
    let api_config = extract_api_config(spec, api_name);
    let functions = extract_functions(spec, api_name)?;
    let schemas = extract_schemas(spec);

    Ok(Manifest {
        apis: vec![api_config],
        functions,
        schemas,
    })
}

// ---------------------------------------------------------------------------
// API config extraction
// ---------------------------------------------------------------------------

fn extract_api_config(spec: &OpenAPI, api_name: &str) -> ApiConfig {
    let base_url = spec
        .servers
        .first()
        .map_or_else(|| "/".to_string(), |s| s.url.clone());

    let auth = spec.components.as_ref().and_then(extract_auth_config);

    ApiConfig {
        name: api_name.to_string(),
        base_url,
        description: spec.info.description.clone(),
        version: Some(spec.info.version.clone()),
        auth,
    }
}

fn extract_auth_config(components: &openapiv3::Components) -> Option<AuthConfig> {
    for (_name, scheme_ref) in &components.security_schemes {
        if let ReferenceOr::Item(scheme) = scheme_ref {
            match scheme {
                SecurityScheme::HTTP {
                    scheme,
                    description: _,
                    bearer_format: _,
                    extensions: _,
                } => {
                    if scheme.eq_ignore_ascii_case("bearer") {
                        return Some(AuthConfig::Bearer {
                            header: "Authorization".to_string(),
                            prefix: "Bearer ".to_string(),
                        });
                    } else if scheme.eq_ignore_ascii_case("basic") {
                        return Some(AuthConfig::Basic);
                    }
                }
                SecurityScheme::APIKey {
                    location: _,
                    name,
                    description: _,
                    extensions: _,
                } => {
                    return Some(AuthConfig::ApiKey {
                        header: name.clone(),
                    });
                }
                _ => {}
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Function extraction
// ---------------------------------------------------------------------------

fn extract_functions(spec: &OpenAPI, api_name: &str) -> Result<Vec<FunctionDef>> {
    let mut functions = Vec::new();

    for (path, method, operation) in spec.operations() {
        let http_method = match method {
            "get" => HttpMethod::Get,
            "post" => HttpMethod::Post,
            "put" => HttpMethod::Put,
            "patch" => HttpMethod::Patch,
            "delete" => HttpMethod::Delete,
            _ => continue, // Skip unsupported methods (options, head, trace)
        };

        let name = derive_function_name(operation.operation_id.as_deref(), method, path);
        let tag = operation.tags.first().cloned();

        let parameters = extract_parameters(&operation.parameters, spec)?;
        let request_body = extract_request_body(operation.request_body.as_ref(), spec)?;
        let response_schema = extract_response_schema(&operation.responses);

        functions.push(FunctionDef {
            name,
            api: api_name.to_string(),
            tag,
            method: http_method,
            path: path.to_string(),
            summary: operation.summary.clone(),
            description: operation.description.clone(),
            deprecated: operation.deprecated,
            parameters,
            request_body,
            response_schema,
        });
    }

    Ok(functions)
}

/// Derive a `snake_case` function name from an `operationId` or method+path.
fn derive_function_name(operation_id: Option<&str>, method: &str, path: &str) -> String {
    operation_id.map_or_else(|| fallback_function_name(method, path), camel_to_snake)
}

/// Convert `camelCase` to `snake_case`.
///
/// Examples:
///   `listPets` -> `list_pets`
///   `getPetById` -> `get_pet_by_id`
///   `HTMLParser` -> `html_parser`
fn camel_to_snake(s: &str) -> String {
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
fn fallback_function_name(method: &str, path: &str) -> String {
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

fn extract_parameters(params: &[ReferenceOr<Parameter>], spec: &OpenAPI) -> Result<Vec<ParamDef>> {
    let mut result = Vec::new();

    for param_ref in params {
        let param = resolve_parameter(param_ref, spec)?;
        let data = param.parameter_data_ref();

        let location = match param {
            Parameter::Query { .. } => ParamLocation::Query,
            Parameter::Header { .. } => ParamLocation::Header,
            Parameter::Path { .. } => ParamLocation::Path,
            Parameter::Cookie { .. } => continue, // Skip cookie params
        };

        let (param_type, default_val, enum_values, format) = extract_param_type_info(&data.format);

        result.push(ParamDef {
            name: data.name.clone(),
            location,
            param_type,
            required: data.required,
            description: data.description.clone(),
            default: default_val,
            enum_values,
            format,
        });
    }

    Ok(result)
}

fn resolve_parameter<'a>(
    param_ref: &'a ReferenceOr<Parameter>,
    spec: &'a OpenAPI,
) -> Result<&'a Parameter> {
    match param_ref {
        ReferenceOr::Item(param) => Ok(param),
        ReferenceOr::Reference { reference } => {
            let name = reference
                .strip_prefix("#/components/parameters/")
                .with_context(|| format!("Unsupported parameter $ref: {reference}"))?;
            let components = spec
                .components
                .as_ref()
                .context("Spec has $ref but no components")?;
            match components.parameters.get(name) {
                Some(ReferenceOr::Item(param)) => Ok(param),
                _ => anyhow::bail!("Could not resolve parameter ref: {reference}"),
            }
        }
    }
}

fn extract_param_type_info(
    format: &ParameterSchemaOrContent,
) -> (
    ParamType,
    Option<serde_json::Value>,
    Option<Vec<String>>,
    Option<String>,
) {
    match format {
        ParameterSchemaOrContent::Schema(schema_ref) => {
            if let ReferenceOr::Item(schema) = schema_ref {
                let param_type = schema_type_to_param_type(&schema.schema_kind);
                let default_val = schema.schema_data.default.clone();
                let enum_values = extract_enum_values(&schema.schema_kind);
                let fmt = extract_format(&schema.schema_kind);
                (param_type, default_val, enum_values, fmt)
            } else {
                (ParamType::String, None, None, None)
            }
        }
        ParameterSchemaOrContent::Content(_) => (ParamType::String, None, None, None),
    }
}

const fn schema_type_to_param_type(kind: &SchemaKind) -> ParamType {
    match kind {
        SchemaKind::Type(Type::Integer(_)) => ParamType::Integer,
        SchemaKind::Type(Type::Number(_)) => ParamType::Number,
        SchemaKind::Type(Type::Boolean(_)) => ParamType::Boolean,
        _ => ParamType::String,
    }
}

fn extract_enum_values(kind: &SchemaKind) -> Option<Vec<String>> {
    if let SchemaKind::Type(Type::String(string_type)) = kind
        && !string_type.enumeration.is_empty()
    {
        let values: Vec<String> = string_type
            .enumeration
            .iter()
            .filter_map(Clone::clone)
            .collect();
        if !values.is_empty() {
            return Some(values);
        }
    }
    None
}

fn extract_request_body(
    body: Option<&ReferenceOr<openapiv3::RequestBody>>,
    spec: &OpenAPI,
) -> Result<Option<RequestBodyDef>> {
    let Some(body) = body else {
        return Ok(None);
    };

    let resolved = resolve_request_body(body, spec)?;

    // Look for application/json content first
    for (content_type, media_type) in &resolved.content {
        if content_type.contains("json") {
            let schema_name = media_type
                .schema
                .as_ref()
                .and_then(extract_ref_name)
                .unwrap_or_else(|| "unknown".to_string());

            return Ok(Some(RequestBodyDef {
                content_type: content_type.clone(),
                schema: schema_name,
                required: resolved.required,
                description: resolved.description.clone(),
            }));
        }
    }

    // Fall back to first content type
    if let Some((content_type, media_type)) = resolved.content.first() {
        let schema_name = media_type
            .schema
            .as_ref()
            .and_then(extract_ref_name)
            .unwrap_or_else(|| "unknown".to_string());

        return Ok(Some(RequestBodyDef {
            content_type: content_type.clone(),
            schema: schema_name,
            required: resolved.required,
            description: resolved.description.clone(),
        }));
    }

    Ok(None)
}

fn resolve_request_body<'a>(
    body_ref: &'a ReferenceOr<openapiv3::RequestBody>,
    spec: &'a OpenAPI,
) -> Result<&'a openapiv3::RequestBody> {
    match body_ref {
        ReferenceOr::Item(body) => Ok(body),
        ReferenceOr::Reference { reference } => {
            let name = reference
                .strip_prefix("#/components/requestBodies/")
                .with_context(|| format!("Unsupported requestBody $ref: {reference}"))?;
            let components = spec
                .components
                .as_ref()
                .context("Spec has $ref but no components")?;
            match components.request_bodies.get(name) {
                Some(ReferenceOr::Item(body)) => Ok(body),
                _ => anyhow::bail!("Could not resolve requestBody ref: {reference}"),
            }
        }
    }
}

/// Extract a schema reference name from a `ReferenceOr<Schema>`.
fn extract_ref_name<T>(ref_or: &ReferenceOr<T>) -> Option<String> {
    match ref_or {
        ReferenceOr::Reference { reference } => reference
            .strip_prefix("#/components/schemas/")
            .map(ToString::to_string),
        ReferenceOr::Item(_) => None,
    }
}

fn extract_response_schema(responses: &openapiv3::Responses) -> Option<String> {
    // Check all 2xx responses for a schema reference
    for code in 200..=299u16 {
        let status = openapiv3::StatusCode::Code(code);
        if let Some(ReferenceOr::Item(response)) = responses.responses.get(&status)
            && let Some(media_type) = response.content.get("application/json")
            && let Some(schema_ref) = &media_type.schema
        {
            if let Some(name) = extract_ref_name(schema_ref) {
                return Some(name);
            }
            if let ReferenceOr::Item(schema) = schema_ref
                && let SchemaKind::Type(Type::Array(arr)) = &schema.schema_kind
                && let Some(items) = &arr.items
                && let Some(name) = extract_ref_name(items)
            {
                return Some(name);
            }
        }
    }

    // Check default response
    if let Some(ReferenceOr::Item(response)) = &responses.default
        && let Some(media_type) = response.content.get("application/json")
        && let Some(schema_ref) = &media_type.schema
        && let Some(name) = extract_ref_name(schema_ref)
    {
        return Some(name);
    }

    None
}

// ---------------------------------------------------------------------------
// Schema extraction
// ---------------------------------------------------------------------------

fn extract_schemas(spec: &OpenAPI) -> Vec<SchemaDef> {
    let Some(components) = &spec.components else {
        return Vec::new();
    };

    let mut schemas = Vec::new();

    for (name, schema_ref) in &components.schemas {
        if let ReferenceOr::Item(schema) = schema_ref
            && let Some(schema_def) = extract_schema_def(name, schema, components)
        {
            schemas.push(schema_def);
        }
    }

    schemas
}

fn extract_schema_def(
    name: &str,
    schema: &Schema,
    components: &openapiv3::Components,
) -> Option<SchemaDef> {
    match &schema.schema_kind {
        SchemaKind::Type(Type::Object(obj)) => {
            let fields: Vec<FieldDef> = obj
                .properties
                .iter()
                .map(|(field_name, field_schema_ref)| {
                    let is_required = obj.required.contains(field_name);
                    extract_field_def(field_name, field_schema_ref, is_required, components)
                })
                .collect();

            Some(SchemaDef {
                name: name.to_string(),
                description: schema.schema_data.description.clone(),
                fields,
            })
        }
        SchemaKind::AllOf { all_of } => {
            let mut properties: Vec<(String, ReferenceOr<Box<Schema>>)> = Vec::new();
            let mut required: Vec<String> = Vec::new();

            for sub_ref in all_of {
                collect_object_properties(sub_ref, components, &mut properties, &mut required);
            }

            let fields: Vec<FieldDef> = properties
                .iter()
                .map(|(field_name, field_schema_ref)| {
                    let is_required = required.contains(field_name);
                    extract_field_def(field_name, field_schema_ref, is_required, components)
                })
                .collect();

            Some(SchemaDef {
                name: name.to_string(),
                description: schema.schema_data.description.clone(),
                fields,
            })
        }
        _ => None, // Only extract object and allOf schemas as SchemaDefs
    }
}

/// Recursively collect properties and required fields from a schema reference,
/// handling both Object types and nested `AllOf` compositions.
fn collect_object_properties(
    schema_ref: &ReferenceOr<Schema>,
    components: &openapiv3::Components,
    properties: &mut Vec<(String, ReferenceOr<Box<Schema>>)>,
    required: &mut Vec<String>,
) {
    let schema = match schema_ref {
        ReferenceOr::Reference { reference } => {
            let schema_name = reference
                .strip_prefix("#/components/schemas/")
                .unwrap_or(reference);
            match components.schemas.get(schema_name) {
                Some(ReferenceOr::Item(s)) => s,
                _ => return,
            }
        }
        ReferenceOr::Item(s) => s,
    };

    match &schema.schema_kind {
        SchemaKind::Type(Type::Object(obj)) => {
            for (name, prop_ref) in &obj.properties {
                properties.push((name.clone(), prop_ref.clone()));
            }
            required.extend(obj.required.iter().cloned());
        }
        SchemaKind::AllOf { all_of } => {
            for sub_ref in all_of {
                collect_object_properties(sub_ref, components, properties, required);
            }
        }
        _ => {}
    }
}

fn extract_field_def(
    name: &str,
    schema_ref: &ReferenceOr<Box<Schema>>,
    required: bool,
    components: &openapiv3::Components,
) -> FieldDef {
    match schema_ref {
        ReferenceOr::Reference { reference } => {
            let schema_name = reference
                .strip_prefix("#/components/schemas/")
                .unwrap_or(reference)
                .to_string();
            // Look up the referenced schema to get its description
            let description = components
                .schemas
                .get(&schema_name)
                .and_then(|s| s.as_item())
                .and_then(|s| s.schema_data.description.clone());
            FieldDef {
                name: name.to_string(),
                field_type: FieldType::Object {
                    schema: schema_name,
                },
                required,
                description,
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
                name: name.to_string(),
                field_type,
                required,
                description: schema.schema_data.description.clone(),
                enum_values,
                nullable,
                format,
            }
        }
    }
}

fn schema_kind_to_field_type(kind: &SchemaKind) -> FieldType {
    match kind {
        SchemaKind::Type(Type::Integer(_)) => FieldType::Integer,
        SchemaKind::Type(Type::Number(_)) => FieldType::Number,
        SchemaKind::Type(Type::Boolean(_)) => FieldType::Boolean,
        SchemaKind::Type(Type::Array(arr)) => {
            let items_type =
                arr.items
                    .as_ref()
                    .map_or(FieldType::String, |items_ref| match items_ref {
                        ReferenceOr::Reference { reference } => {
                            let schema_name = reference
                                .strip_prefix("#/components/schemas/")
                                .unwrap_or(reference);
                            FieldType::Object {
                                schema: schema_name.to_string(),
                            }
                        }
                        ReferenceOr::Item(schema) => schema_kind_to_field_type(&schema.schema_kind),
                    });
            FieldType::Array {
                items: Box::new(items_type),
            }
        }
        SchemaKind::Type(Type::Object(obj)) => {
            if obj.properties.is_empty()
                && let Some(ap) = &obj.additional_properties
            {
                return additional_properties_to_map(ap);
            }
            FieldType::Object {
                schema: "unknown".to_string(),
            }
        }
        _ => FieldType::String, // Fallback for String, Any, OneOf, etc.
    }
}

fn additional_properties_to_map(ap: &openapiv3::AdditionalProperties) -> FieldType {
    match ap {
        openapiv3::AdditionalProperties::Schema(schema_ref) => {
            let value_type = match schema_ref.as_ref() {
                ReferenceOr::Reference { reference } => {
                    let schema_name = reference
                        .strip_prefix("#/components/schemas/")
                        .unwrap_or(reference);
                    FieldType::Object {
                        schema: schema_name.to_string(),
                    }
                }
                ReferenceOr::Item(schema) => schema_kind_to_field_type(&schema.schema_kind),
            };
            FieldType::Map {
                value: Box::new(value_type),
            }
        }
        openapiv3::AdditionalProperties::Any(true) => FieldType::Map {
            value: Box::new(FieldType::String),
        },
        openapiv3::AdditionalProperties::Any(false) => FieldType::Object {
            schema: "unknown".to_string(),
        },
    }
}

fn extract_format(kind: &SchemaKind) -> Option<String> {
    match kind {
        SchemaKind::Type(Type::String(s)) => variant_or_to_string(&s.format),
        SchemaKind::Type(Type::Integer(i)) => variant_or_to_string(&i.format),
        SchemaKind::Type(Type::Number(n)) => variant_or_to_string(&n.format),
        _ => None,
    }
}

fn variant_or_to_string<T: serde::Serialize>(
    v: &openapiv3::VariantOrUnknownOrEmpty<T>,
) -> Option<String> {
    match v {
        openapiv3::VariantOrUnknownOrEmpty::Item(item) => serde_json::to_value(item)
            .ok()
            .and_then(|v| v.as_str().map(String::from)),
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) => Some(s.clone()),
        openapiv3::VariantOrUnknownOrEmpty::Empty => None,
    }
}

fn extract_field_enum_values(kind: &SchemaKind) -> Option<Vec<String>> {
    if let SchemaKind::Type(Type::String(string_type)) = kind
        && !string_type.enumeration.is_empty()
    {
        let values: Vec<String> = string_type
            .enumeration
            .iter()
            .filter_map(Clone::clone)
            .collect();
        if !values.is_empty() {
            return Some(values);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::codegen::manifest::ParamLocation;

    #[test]
    fn test_load_spec_from_file() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert_eq!(spec.info.title, "Petstore");
        assert!(!spec.paths.paths.is_empty());
    }

    #[test]
    fn test_load_spec_from_file_info() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert_eq!(spec.info.version, "1.0.0");
        assert!(spec.info.description.is_some());
    }

    #[test]
    fn test_load_spec_from_file_servers() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert_eq!(spec.servers.len(), 1);
        assert_eq!(spec.servers[0].url, "https://petstore.example.com/v1");
    }

    #[test]
    fn test_load_spec_from_file_paths() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert!(spec.paths.paths.contains_key("/pets"));
        assert!(spec.paths.paths.contains_key("/pets/{petId}"));
    }

    #[test]
    fn test_load_spec_from_file_schemas() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let components = spec.components.as_ref().unwrap();
        assert!(components.schemas.contains_key("Pet"));
        assert!(components.schemas.contains_key("NewPet"));
    }

    #[test]
    fn test_load_spec_from_file_security() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let components = spec.components.as_ref().unwrap();
        assert!(components.security_schemes.contains_key("bearerAuth"));
    }

    #[test]
    fn test_load_spec_from_file_tags() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert_eq!(spec.tags.len(), 1);
        assert_eq!(spec.tags[0].name, "pets");
    }

    #[test]
    fn test_load_spec_nonexistent_file() {
        let result = load_spec_from_file(Path::new("testdata/nonexistent.yaml"));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // camel_to_snake tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_camel_to_snake_basic() {
        assert_eq!(camel_to_snake("listPets"), "list_pets");
        assert_eq!(camel_to_snake("getPetById"), "get_pet_by_id");
        assert_eq!(camel_to_snake("createPet"), "create_pet");
    }

    #[test]
    fn test_camel_to_snake_already_snake() {
        assert_eq!(camel_to_snake("list_pets"), "list_pets");
    }

    #[test]
    fn test_camel_to_snake_uppercase_run() {
        assert_eq!(camel_to_snake("HTMLParser"), "html_parser");
        assert_eq!(camel_to_snake("getAPIKey"), "get_api_key");
    }

    #[test]
    fn test_camel_to_snake_single_word() {
        assert_eq!(camel_to_snake("pets"), "pets");
        assert_eq!(camel_to_snake("Pets"), "pets");
    }

    // -----------------------------------------------------------------------
    // fallback_function_name tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_fallback_function_name() {
        assert_eq!(fallback_function_name("get", "/pets"), "get_pets");
        assert_eq!(
            fallback_function_name("get", "/pets/{petId}"),
            "get_pets_by_pet_id"
        );
        assert_eq!(fallback_function_name("post", "/pets"), "post_pets");
        assert_eq!(
            fallback_function_name("delete", "/users/{userId}/posts/{postId}"),
            "delete_users_by_user_id_posts_by_post_id"
        );
    }

    // -----------------------------------------------------------------------
    // spec_to_manifest tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_spec_to_manifest() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        // API config
        assert_eq!(manifest.apis.len(), 1);
        assert_eq!(manifest.apis[0].name, "petstore");
        assert!(!manifest.apis[0].base_url.is_empty());
        assert_eq!(manifest.apis[0].base_url, "https://petstore.example.com/v1");

        // Functions - should have: list_pets, create_pet, get_pet_by_id
        assert!(
            manifest.functions.len() >= 3,
            "Expected at least 3 functions, got {}",
            manifest.functions.len()
        );
        for func in &manifest.functions {
            assert!(
                func.summary.is_some() || func.description.is_some(),
                "Function {} missing docs",
                func.name
            );
        }

        // Should have path params, query params
        let has_path_param = manifest.functions.iter().any(|f| {
            f.parameters
                .iter()
                .any(|p| matches!(p.location, ParamLocation::Path))
        });
        assert!(has_path_param, "Expected at least one path parameter");

        let has_query_param = manifest.functions.iter().any(|f| {
            f.parameters
                .iter()
                .any(|p| matches!(p.location, ParamLocation::Query))
        });
        assert!(has_query_param, "Expected at least one query parameter");

        // Schemas
        assert!(!manifest.schemas.is_empty(), "Expected at least one schema");
        let pet_schema = manifest
            .schemas
            .iter()
            .find(|s| s.name == "Pet")
            .expect("Expected Pet schema");
        assert!(!pet_schema.fields.is_empty());

        // Auth
        assert!(
            manifest.apis[0].auth.is_some(),
            "Expected auth configuration"
        );
    }

    #[test]
    fn test_spec_to_manifest_function_names() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let names: Vec<&str> = manifest.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(
            names.contains(&"list_pets"),
            "Expected list_pets, got: {names:?}"
        );
        assert!(
            names.contains(&"create_pet"),
            "Expected create_pet, got: {names:?}"
        );
        assert!(
            names.contains(&"get_pet_by_id"),
            "Expected get_pet_by_id, got: {names:?}"
        );
    }

    #[test]
    fn test_spec_to_manifest_function_methods() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let list_pets = manifest
            .functions
            .iter()
            .find(|f| f.name == "list_pets")
            .unwrap();
        assert_eq!(list_pets.method, HttpMethod::Get);

        let create_pet = manifest
            .functions
            .iter()
            .find(|f| f.name == "create_pet")
            .unwrap();
        assert_eq!(create_pet.method, HttpMethod::Post);
    }

    #[test]
    fn test_spec_to_manifest_query_params() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let list_pets = manifest
            .functions
            .iter()
            .find(|f| f.name == "list_pets")
            .unwrap();

        assert_eq!(list_pets.parameters.len(), 2);

        let status_param = list_pets
            .parameters
            .iter()
            .find(|p| p.name == "status")
            .unwrap();
        assert_eq!(status_param.location, ParamLocation::Query);
        assert_eq!(status_param.param_type, ParamType::String);
        assert!(!status_param.required);
        assert!(status_param.enum_values.is_some());
        let enums = status_param.enum_values.as_ref().unwrap();
        assert_eq!(enums.len(), 3);
        assert!(enums.contains(&"available".to_string()));

        let limit_param = list_pets
            .parameters
            .iter()
            .find(|p| p.name == "limit")
            .unwrap();
        assert_eq!(limit_param.location, ParamLocation::Query);
        assert_eq!(limit_param.param_type, ParamType::Integer);
        assert!(!limit_param.required);
    }

    #[test]
    fn test_spec_to_manifest_path_params() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let get_pet = manifest
            .functions
            .iter()
            .find(|f| f.name == "get_pet_by_id")
            .unwrap();

        assert_eq!(get_pet.parameters.len(), 1);
        let pet_id_param = &get_pet.parameters[0];
        assert_eq!(pet_id_param.name, "petId");
        assert_eq!(pet_id_param.location, ParamLocation::Path);
        assert!(pet_id_param.required);
    }

    #[test]
    fn test_spec_to_manifest_request_body() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let create_pet = manifest
            .functions
            .iter()
            .find(|f| f.name == "create_pet")
            .unwrap();

        let body = create_pet.request_body.as_ref().unwrap();
        assert_eq!(body.content_type, "application/json");
        assert_eq!(body.schema, "NewPet");
        assert!(body.required);
    }

    #[test]
    fn test_spec_to_manifest_response_schema() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let list_pets = manifest
            .functions
            .iter()
            .find(|f| f.name == "list_pets")
            .unwrap();
        // list_pets returns array of Pet, so response_schema should be "Pet"
        assert_eq!(list_pets.response_schema.as_deref(), Some("Pet"));

        let get_pet = manifest
            .functions
            .iter()
            .find(|f| f.name == "get_pet_by_id")
            .unwrap();
        assert_eq!(get_pet.response_schema.as_deref(), Some("Pet"));

        let create_pet = manifest
            .functions
            .iter()
            .find(|f| f.name == "create_pet")
            .unwrap();
        assert_eq!(create_pet.response_schema.as_deref(), Some("Pet"));
    }

    #[test]
    fn test_spec_to_manifest_tags() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        for func in &manifest.functions {
            assert_eq!(
                func.tag.as_deref(),
                Some("pets"),
                "Function {} should be tagged 'pets'",
                func.name
            );
        }
    }

    #[test]
    fn test_spec_to_manifest_pet_schema() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let pet = manifest
            .schemas
            .iter()
            .find(|s| s.name == "Pet")
            .expect("Pet schema missing");

        assert!(pet.description.is_some());
        assert_eq!(pet.fields.len(), 4);

        let id_field = pet.fields.iter().find(|f| f.name == "id").unwrap();
        assert_eq!(id_field.field_type, FieldType::String);
        assert!(id_field.required);

        let name_field = pet.fields.iter().find(|f| f.name == "name").unwrap();
        assert_eq!(name_field.field_type, FieldType::String);
        assert!(name_field.required);

        let status_field = pet.fields.iter().find(|f| f.name == "status").unwrap();
        assert_eq!(status_field.field_type, FieldType::String);
        assert!(status_field.required);
        assert!(status_field.enum_values.is_some());
        let enums = status_field.enum_values.as_ref().unwrap();
        assert!(enums.contains(&"available".to_string()));
        assert!(enums.contains(&"pending".to_string()));
        assert!(enums.contains(&"sold".to_string()));

        let tag_field = pet.fields.iter().find(|f| f.name == "tag").unwrap();
        assert_eq!(tag_field.field_type, FieldType::String);
        assert!(!tag_field.required);
    }

    #[test]
    fn test_spec_to_manifest_new_pet_schema() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let new_pet = manifest
            .schemas
            .iter()
            .find(|s| s.name == "NewPet")
            .expect("NewPet schema missing");

        assert_eq!(new_pet.fields.len(), 2);

        let name_field = new_pet.fields.iter().find(|f| f.name == "name").unwrap();
        assert!(name_field.required);

        let tag_field = new_pet.fields.iter().find(|f| f.name == "tag").unwrap();
        assert!(!tag_field.required);
    }

    #[test]
    fn test_spec_to_manifest_auth() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let auth = manifest.apis[0].auth.as_ref().unwrap();
        assert_eq!(
            *auth,
            AuthConfig::Bearer {
                header: "Authorization".to_string(),
                prefix: "Bearer ".to_string(),
            }
        );
    }

    #[test]
    fn test_spec_to_manifest_api_metadata() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let api = &manifest.apis[0];
        assert_eq!(api.name, "petstore");
        assert!(api.description.is_some());
        assert_eq!(api.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn test_spec_to_manifest_no_deprecated() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        for func in &manifest.functions {
            assert!(
                !func.deprecated,
                "Function {} should not be deprecated",
                func.name
            );
        }
    }

    #[test]
    fn test_manifest_serializes_to_json() {
        let spec = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "petstore").unwrap();

        let json =
            serde_json::to_string_pretty(&manifest).expect("Manifest should serialize to JSON");
        let roundtripped: Manifest =
            serde_json::from_str(&json).expect("JSON should deserialize back");

        assert_eq!(roundtripped.apis.len(), manifest.apis.len());
        assert_eq!(roundtripped.functions.len(), manifest.functions.len());
        assert_eq!(roundtripped.schemas.len(), manifest.schemas.len());
    }

    // -----------------------------------------------------------------------
    // Advanced spec tests (allOf, nullable, format, additionalProperties)
    // -----------------------------------------------------------------------

    #[test]
    fn test_allof_schema_extraction() {
        let spec = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "advanced").unwrap();

        let resource = manifest
            .schemas
            .iter()
            .find(|s| s.name == "Resource")
            .expect("Resource schema missing");

        let field_names: Vec<&str> = resource.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(
            field_names.contains(&"id"),
            "Missing id from BaseResource. Got: {field_names:?}"
        );
        assert!(
            field_names.contains(&"created_at"),
            "Missing created_at from BaseResource. Got: {field_names:?}"
        );
        assert!(
            field_names.contains(&"name"),
            "Missing name from inline. Got: {field_names:?}"
        );
        assert!(
            field_names.contains(&"metadata"),
            "Missing metadata. Got: {field_names:?}"
        );
    }

    #[test]
    fn test_nullable_fields() {
        let spec = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "advanced").unwrap();

        let resource = manifest
            .schemas
            .iter()
            .find(|s| s.name == "Resource")
            .expect("Resource schema missing");

        let updated_at = resource
            .fields
            .iter()
            .find(|f| f.name == "updated_at")
            .unwrap();
        assert!(updated_at.nullable, "updated_at should be nullable");

        let description_field = resource
            .fields
            .iter()
            .find(|f| f.name == "description")
            .unwrap();
        assert!(description_field.nullable, "description should be nullable");

        let name = resource.fields.iter().find(|f| f.name == "name").unwrap();
        assert!(!name.nullable, "name should not be nullable");
    }

    #[test]
    fn test_format_extraction() {
        let spec = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "advanced").unwrap();

        let resource = manifest
            .schemas
            .iter()
            .find(|s| s.name == "Resource")
            .expect("Resource schema missing");

        let id_field = resource.fields.iter().find(|f| f.name == "id").unwrap();
        assert_eq!(id_field.format.as_deref(), Some("uuid"));

        let created_at = resource
            .fields
            .iter()
            .find(|f| f.name == "created_at")
            .unwrap();
        assert_eq!(created_at.format.as_deref(), Some("date-time"));

        let error = manifest.schemas.iter().find(|s| s.name == "Error").unwrap();
        let code_field = error.fields.iter().find(|f| f.name == "code").unwrap();
        assert_eq!(code_field.format.as_deref(), Some("int32"));
    }

    #[test]
    fn test_additional_properties_map() {
        let spec = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "advanced").unwrap();

        let resource = manifest
            .schemas
            .iter()
            .find(|s| s.name == "Resource")
            .expect("Resource schema missing");

        let metadata = resource
            .fields
            .iter()
            .find(|f| f.name == "metadata")
            .unwrap();
        assert_eq!(
            metadata.field_type,
            FieldType::Map {
                value: Box::new(FieldType::String)
            },
            "metadata should be Map<string>"
        );
    }

    #[test]
    fn test_header_params_extracted() {
        let spec = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "advanced").unwrap();

        let list = manifest
            .functions
            .iter()
            .find(|f| f.name == "list_resources")
            .unwrap();
        let header_param = list.parameters.iter().find(|p| p.name == "X-Request-ID");
        assert!(
            header_param.is_some(),
            "Header param X-Request-ID should be extracted"
        );
        assert_eq!(header_param.unwrap().location, ParamLocation::Header);

        let update = manifest
            .functions
            .iter()
            .find(|f| f.name == "update_resource")
            .unwrap();
        let idemp = update
            .parameters
            .iter()
            .find(|p| p.name == "X-Idempotency-Key");
        assert!(
            idemp.is_some(),
            "Header param X-Idempotency-Key should be extracted"
        );
        assert!(idemp.unwrap().required);
    }

    #[test]
    fn test_allof_required_field_inheritance() {
        let spec = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "advanced").unwrap();

        let resource = manifest
            .schemas
            .iter()
            .find(|s| s.name == "Resource")
            .expect("Resource schema missing");

        // Inherited from BaseResource required: [id, created_at]
        let id_field = resource.fields.iter().find(|f| f.name == "id").unwrap();
        assert!(
            id_field.required,
            "id should be required (inherited from BaseResource)"
        );

        let created_at = resource
            .fields
            .iter()
            .find(|f| f.name == "created_at")
            .unwrap();
        assert!(
            created_at.required,
            "created_at should be required (inherited from BaseResource)"
        );

        // From inline schema required: [name]
        let name_field = resource.fields.iter().find(|f| f.name == "name").unwrap();
        assert!(
            name_field.required,
            "name should be required (from inline schema)"
        );

        // Not in any required list
        let description_field = resource
            .fields
            .iter()
            .find(|f| f.name == "description")
            .unwrap();
        assert!(
            !description_field.required,
            "description should not be required"
        );

        let metadata_field = resource
            .fields
            .iter()
            .find(|f| f.name == "metadata")
            .unwrap();
        assert!(!metadata_field.required, "metadata should not be required");
    }

    #[test]
    fn test_allof_three_levels() {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0.0"
paths: {}
components:
  schemas:
    A:
      type: object
      required: [a_field]
      properties:
        a_field:
          type: string
    B:
      allOf:
        - $ref: "#/components/schemas/A"
        - type: object
          required: [b_field]
          properties:
            b_field:
              type: integer
    C:
      allOf:
        - $ref: "#/components/schemas/B"
        - type: object
          properties:
            c_field:
              type: boolean
"##;
        let spec: OpenAPI = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(&spec, "test").unwrap();

        let c_schema = manifest
            .schemas
            .iter()
            .find(|s| s.name == "C")
            .expect("C schema missing");

        // Should have all 3 fields from the three levels
        let field_names: Vec<&str> = c_schema.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(
            field_names.contains(&"a_field"),
            "Missing a_field from A. Got: {field_names:?}"
        );
        assert!(
            field_names.contains(&"b_field"),
            "Missing b_field from B. Got: {field_names:?}"
        );
        assert!(
            field_names.contains(&"c_field"),
            "Missing c_field from C. Got: {field_names:?}"
        );

        // a_field should be required (from A)
        let a_field = c_schema
            .fields
            .iter()
            .find(|f| f.name == "a_field")
            .unwrap();
        assert!(a_field.required, "a_field should be required (from A)");

        // b_field should be required (from B)
        let b_field = c_schema
            .fields
            .iter()
            .find(|f| f.name == "b_field")
            .unwrap();
        assert!(b_field.required, "b_field should be required (from B)");

        // c_field should NOT be required (not in any required list)
        let c_field = c_schema
            .fields
            .iter()
            .find(|f| f.name == "c_field")
            .unwrap();
        assert!(!c_field.required, "c_field should not be required");
    }

    #[test]
    fn test_additional_properties_object_ref() {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0.0"
paths:
  /things:
    get:
      operationId: getThings
      responses:
        "200":
          description: ok
components:
  schemas:
    Container:
      type: object
      required: [items]
      properties:
        items:
          type: object
          additionalProperties:
            $ref: "#/components/schemas/Item"
    Item:
      type: object
      properties:
        name:
          type: string
"##;
        let spec: OpenAPI = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(&spec, "test").unwrap();

        let container = manifest
            .schemas
            .iter()
            .find(|s| s.name == "Container")
            .expect("Container schema missing");

        let items_field = container.fields.iter().find(|f| f.name == "items").unwrap();

        // items field should be a Map with value type Object { schema: "Item" }
        assert_eq!(
            items_field.field_type,
            FieldType::Map {
                value: Box::new(FieldType::Object {
                    schema: "Item".to_string()
                })
            },
            "items should be Map<Item> via additionalProperties $ref"
        );

        // Item schema should also be extracted
        let item = manifest
            .schemas
            .iter()
            .find(|s| s.name == "Item")
            .expect("Item schema should be extracted");
        assert!(
            item.fields.iter().any(|f| f.name == "name"),
            "Item should have a name field"
        );
    }

    #[test]
    fn test_response_schema_204_no_content() {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0.0"
paths:
  /things/{id}:
    delete:
      operationId: deleteThing
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
      responses:
        "204":
          description: Deleted
"##;
        let spec: OpenAPI = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(&spec, "test").unwrap();

        let delete_thing = manifest
            .functions
            .iter()
            .find(|f| f.name == "delete_thing")
            .expect("delete_thing function missing");

        assert_eq!(
            delete_thing.response_schema, None,
            "204 No Content should have no response schema"
        );
    }

    #[test]
    fn test_response_schema_prefers_lower_2xx() {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0.0"
paths:
  /things:
    post:
      operationId: createThing
      responses:
        "201":
          description: Created
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/CreatedThing"
        "200":
          description: Already existed
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/ExistingThing"
components:
  schemas:
    CreatedThing:
      type: object
      properties:
        id:
          type: string
    ExistingThing:
      type: object
      properties:
        id:
          type: string
"##;
        let spec: OpenAPI = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(&spec, "test").unwrap();

        let create_thing = manifest
            .functions
            .iter()
            .find(|f| f.name == "create_thing")
            .expect("create_thing function missing");

        // The loop iterates 200..=299, so 200 is found first
        assert_eq!(
            create_thing.response_schema.as_deref(),
            Some("ExistingThing"),
            "Should prefer 200 over 201 since the loop checks lower codes first"
        );
    }

    #[test]
    fn test_response_schema_202_accepted() {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0.0"
paths:
  /jobs:
    post:
      operationId: createJob
      responses:
        "202":
          description: Accepted
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Job"
components:
  schemas:
    Job:
      type: object
      properties:
        id:
          type: string
"##;
        let spec: OpenAPI = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(&spec, "test").unwrap();

        let create_job = manifest
            .functions
            .iter()
            .find(|f| f.name == "create_job")
            .unwrap();
        assert_eq!(
            create_job.response_schema.as_deref(),
            Some("Job"),
            "Should extract schema from 202 response"
        );
    }

    #[test]
    fn test_param_format_extraction() {
        let spec = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&spec, "advanced").unwrap();

        let get_resource = manifest
            .functions
            .iter()
            .find(|f| f.name == "get_resource")
            .expect("get_resource function missing");

        let id_param = get_resource
            .parameters
            .iter()
            .find(|p| p.name == "id")
            .expect("id param missing");

        assert_eq!(id_param.format.as_deref(), Some("uuid"));
    }
}
