//! Spec loading, version sniffing, and dialect validation.
//!
//! The document traversal lives in [`super::walker`]; this module owns the
//! raw-text boundaries (file/URL loading, YAML/JSON tolerance), version
//! routing, and per-dialect validation. Specs are parsed to a
//! [`serde_json::Value`] once and stay unnormalized until
//! [`spec_to_manifest`] runs the pipeline.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use super::manifest::Manifest;

/// The `OpenAPI` dialect a document declares.
///
/// 3.2.x routes to [`SpecVersion::V31Plus`] as well: the two dialects share
/// the JSON Schema 2020-12 data model, and 3.2-only constructs degrade during
/// typed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecVersion {
    /// `openapi: "3.0.*"` documents, validated with `openapiv3`.
    V30,
    /// `openapi: "3.1.*"` and `"3.2.*"` documents, validated with `oas3`.
    V31Plus,
}

/// Load a spec from a local YAML or JSON file, returning the raw document.
pub fn load_spec_from_file(path: &Path) -> Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    parse_spec_text(&content)
        .with_context(|| format!("Failed to parse OpenAPI spec from {}", path.display()))
}

/// Fetch and parse a spec from a URL, returning the raw document.
pub async fn load_spec_from_url(url: &str) -> Result<Value> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("Failed to fetch spec from {url}"))?;

    let content = response
        .text()
        .await
        .with_context(|| format!("Failed to read response body from {url}"))?;

    parse_spec_text(&content).with_context(|| format!("Failed to parse OpenAPI spec from {url}"))
}

/// Load a spec from a file path or HTTP(S) URL, returning the raw document.
/// Files are read from disk; URLs are fetched over the network.
pub async fn load_spec(path_or_url: &str) -> Result<Value> {
    if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
        load_spec_from_url(path_or_url).await
    } else {
        load_spec_from_file(Path::new(path_or_url))
    }
}

/// Detect the `OpenAPI` dialect of a parsed document.
///
/// `"3.0.*"` maps to [`SpecVersion::V30`]; `"3.1.*"` and `"3.2.*"` map to
/// [`SpecVersion::V31Plus`]. A top-level `swagger` key produces the friendly
/// Swagger 2.0 error. Missing or invalid version strings produce an error that
/// names the observed value.
pub fn sniff_version(doc: &Value) -> Result<SpecVersion> {
    let _ = doc;
    todo!("not implemented")
}

/// Validate a document against its dialect's schema.
///
/// `V30` documents deserialize through `openapiv3::OpenAPI`; `V31Plus`
/// documents deserialize through `oas3::OpenApiV3Spec` (bypassing its
/// `validate_version` check so 3.2.x is accepted). Validation errors surface
/// the typed crates' precise messages.
pub fn validate(doc: &Value, version: SpecVersion) -> Result<()> {
    let _ = (doc, version);
    todo!("not implemented")
}

/// Convert a validated spec document into a [`Manifest`].
///
/// Runs the pipeline: validate (dialect-aware), normalize 2020-12 schema
/// nodes, then extract via the walker. Callers are `generate.rs` and tests.
pub fn spec_to_manifest(
    doc: &Value,
    api_name: &str,
    spec_source: Option<&str>,
) -> Result<Manifest> {
    let _ = (doc, api_name, spec_source);
    todo!("not implemented")
}

/// Parse raw spec text as YAML first (a superset of JSON), then JSON.
fn parse_spec_text(content: &str) -> Result<Value> {
    serde_yaml::from_str::<Value>(content).or_else(|yaml_err| {
        serde_json::from_str::<Value>(content)
            .map_err(|json_err| anyhow::anyhow!("YAML error: {yaml_err}; JSON error: {json_err}"))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::codegen::manifest::{AuthConfig, FieldType, HttpMethod, ParamLocation, ParamType};
    use crate::codegen::walker::{camel_to_snake, fallback_function_name};
    use serde_json::json;

    // -----------------------------------------------------------------------
    // Version sniffing
    // -----------------------------------------------------------------------

    #[test]
    fn test_sniff_version_routes_30() {
        for v in ["3.0.0", "3.0.3", "3.0.4"] {
            let doc = json!({"openapi": v, "info": {"title": "T", "version": "1"}});
            assert_eq!(sniff_version(&doc).unwrap(), SpecVersion::V30, "for {v}");
        }
    }

    #[test]
    fn test_sniff_version_routes_31_and_32_to_v31plus() {
        for v in ["3.1.0", "3.1.1", "3.2.0"] {
            let doc = json!({"openapi": v, "info": {"title": "T", "version": "1"}});
            assert_eq!(
                sniff_version(&doc).unwrap(),
                SpecVersion::V31Plus,
                "for {v}"
            );
        }
    }

    #[test]
    fn test_sniff_version_rejects_swagger_2_with_friendly_error() {
        let doc = json!({"swagger": "2.0", "info": {"title": "T", "version": "1"}});
        let err = sniff_version(&doc).unwrap_err().to_string();
        assert!(
            err.contains("Swagger 2.0 is not supported; convert to OpenAPI 3.x"),
            "error should name the migration path, got: {err}"
        );
    }

    #[test]
    fn test_sniff_version_rejects_missing_version() {
        let doc = json!({"info": {"title": "T", "version": "1"}});
        let err = sniff_version(&doc).unwrap_err().to_string();
        assert!(
            err.contains("no `openapi` version"),
            "error should describe the missing version, got: {err}"
        );
    }

    #[test]
    fn test_sniff_version_rejects_unknown_version_and_names_it() {
        let doc = json!({"openapi": "4.0.0", "info": {"title": "T", "version": "1"}});
        let err = sniff_version(&doc).unwrap_err().to_string();
        assert!(
            err.contains("4.0.0"),
            "error should name the observed version, got: {err}"
        );
    }

    #[test]
    fn test_sniff_version_rejects_non_string_version() {
        let doc = json!({"openapi": 3, "info": {"title": "T", "version": "1"}});
        let err = sniff_version(&doc).unwrap_err().to_string();
        assert!(
            err.contains('3'),
            "error should name the observed value, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_accepts_valid_30_document() {
        let doc = json!({
            "openapi": "3.0.3",
            "info": {"title": "T", "version": "1.0.0"},
            "paths": {}
        });
        assert!(validate(&doc, SpecVersion::V30).is_ok());
    }

    #[test]
    fn test_validate_accepts_valid_31_document() {
        let doc = json!({
            "openapi": "3.1.0",
            "info": {"title": "T", "version": "1.0.0"},
            "paths": {}
        });
        assert!(validate(&doc, SpecVersion::V31Plus).is_ok());
    }

    #[test]
    fn test_validate_accepts_valid_32_document() {
        let doc = json!({
            "openapi": "3.2.0",
            "info": {"title": "T", "version": "1.0.0"},
            "paths": {}
        });
        assert!(validate(&doc, SpecVersion::V31Plus).is_ok());
    }

    #[test]
    fn test_validate_rejects_malformed_30_document() {
        let doc = json!({
            "openapi": "3.0.3",
            "info": {"title": "T", "version": "1.0.0"},
            "paths": {},
            "servers": "not-a-list"
        });
        assert!(
            validate(&doc, SpecVersion::V30).is_err(),
            "malformed 3.0 document must fail typed validation"
        );
    }

    #[test]
    fn test_validate_rejects_malformed_31_document() {
        let doc = json!({
            "openapi": "3.1.0",
            "info": {"title": "T", "version": "1.0.0"},
            "paths": {},
            "servers": "not-a-list"
        });
        assert!(
            validate(&doc, SpecVersion::V31Plus).is_err(),
            "malformed 3.1 document must fail typed validation"
        );
    }

    #[test]
    fn test_validate_31plus_bypasses_version_range_check() {
        // oas3's own validate_version pins ~3.1; the pipeline must accept 3.2
        // documents without that check.
        let doc = json!({
            "openapi": "3.2.0",
            "info": {"title": "T", "version": "1.0.0"},
            "paths": {}
        });
        assert!(
            validate(&doc, SpecVersion::V31Plus).is_ok(),
            "3.2 documents must pass validation"
        );
    }

    // -----------------------------------------------------------------------
    // Loading
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_spec_from_file_yaml() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert_eq!(doc["openapi"], json!("3.0.3"));
        assert_eq!(doc["info"]["title"], json!("Petstore"));
        assert_eq!(doc["info"]["version"], json!("1.0.0"));
        assert!(doc["paths"].get("/pets").is_some());
        assert!(doc["components"]["schemas"].get("Pet").is_some());
        assert!(
            doc["components"]["securitySchemes"]
                .get("bearerAuth")
                .is_some()
        );
    }

    #[test]
    fn test_load_spec_from_file_json() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.json")).unwrap();
        assert_eq!(doc["openapi"], json!("3.0.3"));
        assert_eq!(doc["info"]["title"], json!("Petstore"));
    }

    #[test]
    fn test_load_spec_nonexistent_file() {
        let result = load_spec_from_file(Path::new("testdata/nonexistent.yaml"));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // spec_to_manifest
    // -----------------------------------------------------------------------

    #[test]
    fn test_spec_to_manifest_from_30_petstore() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

        assert_eq!(manifest.apis.len(), 1);
        assert_eq!(manifest.apis[0].name, "petstore");
        assert_eq!(manifest.apis[0].base_url, "https://petstore.example.com/v1");
        assert!(manifest.apis[0].auth.is_some());

        let names: Vec<&str> = manifest.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"list_pets"), "got: {names:?}");
        assert!(names.contains(&"create_pet"), "got: {names:?}");
        assert!(names.contains(&"get_pet_by_id"), "got: {names:?}");

        let pet = manifest.schemas.iter().find(|s| s.name == "Pet").unwrap();
        assert_eq!(pet.fields.len(), 4);
    }

    #[test]
    fn test_spec_to_manifest_from_31_petstore() {
        let doc = load_spec_from_file(Path::new("testdata/petstore_31.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

        assert_eq!(manifest.apis.len(), 1);
        assert_eq!(manifest.apis[0].name, "petstore");
        assert_eq!(manifest.apis[0].base_url, "https://petstore.example.com/v1");
        assert!(manifest.apis[0].auth.is_some());

        let names: Vec<&str> = manifest.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"list_pets"), "got: {names:?}");
        assert!(names.contains(&"create_pet"), "got: {names:?}");
        assert!(names.contains(&"get_pet_by_id"), "got: {names:?}");

        let pet = manifest.schemas.iter().find(|s| s.name == "Pet").unwrap();
        assert_eq!(pet.fields.len(), 4);
    }

    #[test]
    fn test_spec_to_manifest_from_32_petstore() {
        let doc = load_spec_from_file(Path::new("testdata/petstore_32.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

        assert_eq!(manifest.apis.len(), 1);
        assert_eq!(manifest.apis[0].base_url, "https://petstore.example.com/v1");
        let names: Vec<&str> = manifest.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"list_pets"), "got: {names:?}");
        assert!(names.contains(&"create_pet"), "got: {names:?}");
        assert!(names.contains(&"get_pet_by_id"), "got: {names:?}");
    }

    #[test]
    fn test_spec_to_manifest_31_nullability_matches_30() {
        // The 3.1 fixture expresses nullability with `type: [T, "null"]`; the
        // 3.0 twin uses `nullable: true`. Both must render nullable fields.
        let doc_30 = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let doc_31 = load_spec_from_file(Path::new("testdata/advanced_31.yaml")).unwrap();
        let m30 = spec_to_manifest(&doc_30, "advanced", None).unwrap();
        let m31 = spec_to_manifest(&doc_31, "advanced", None).unwrap();

        let updated_30 = m30
            .schemas
            .iter()
            .find(|s| s.name == "Resource")
            .unwrap()
            .fields
            .iter()
            .find(|f| f.name == "updated_at")
            .unwrap();
        let updated_31 = m31
            .schemas
            .iter()
            .find(|s| s.name == "Resource")
            .unwrap()
            .fields
            .iter()
            .find(|f| f.name == "updated_at")
            .unwrap();
        assert!(updated_30.nullable && updated_31.nullable);
        assert_eq!(updated_30.field_type, updated_31.field_type);

        let name_30 = m30
            .schemas
            .iter()
            .find(|s| s.name == "Resource")
            .unwrap()
            .fields
            .iter()
            .find(|f| f.name == "name")
            .unwrap();
        let name_31 = m31
            .schemas
            .iter()
            .find(|s| s.name == "Resource")
            .unwrap()
            .fields
            .iter()
            .find(|f| f.name == "name")
            .unwrap();
        assert!(!name_30.nullable && !name_31.nullable);
    }

    #[test]
    fn test_spec_to_manifest_rejects_swagger_2_document() {
        let doc = json!({"swagger": "2.0", "info": {"title": "T", "version": "1"}});
        let err = spec_to_manifest(&doc, "test", None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Swagger 2.0 is not supported; convert to OpenAPI 3.x"),
            "error should name the migration path, got: {err}"
        );
    }

    #[test]
    fn test_spec_to_manifest_rejects_malformed_document() {
        let doc = json!({
            "openapi": "3.1.0",
            "info": {"title": "T", "version": "1.0.0"},
            "paths": {},
            "servers": "not-a-list"
        });
        assert!(
            spec_to_manifest(&doc, "test", None).is_err(),
            "malformed documents must fail validation before extraction"
        );
    }

    // -----------------------------------------------------------------------
    // Regression tests (ported from the openapiv3-era suite; re-pointed at
    // the Value-based API). Behavior contracts unchanged.
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_spec_from_file() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert_eq!(doc["info"]["title"], serde_json::json!("Petstore"));
        assert!(doc["paths"].as_object().is_some_and(|p| !p.is_empty()));
    }

    #[test]
    fn test_load_spec_from_file_info() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert_eq!(doc["info"]["version"], serde_json::json!("1.0.0"));
        assert!(doc["info"]["description"].is_string());
    }

    #[test]
    fn test_load_spec_from_file_servers() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert_eq!(doc["servers"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            doc["servers"][0]["url"],
            serde_json::json!("https://petstore.example.com/v1")
        );
    }

    #[test]
    fn test_load_spec_from_file_paths() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert!(doc["paths"].get("/pets").is_some());
        assert!(doc["paths"].get("/pets/{petId}").is_some());
    }

    #[test]
    fn test_load_spec_from_file_schemas() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert!(doc["components"]["schemas"].get("Pet").is_some());
        assert!(doc["components"]["schemas"].get("NewPet").is_some());
    }

    #[test]
    fn test_load_spec_from_file_security() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert!(
            doc["components"]["securitySchemes"]
                .get("bearerAuth")
                .is_some()
        );
    }

    #[test]
    fn test_load_spec_from_file_tags() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        assert_eq!(doc["tags"].as_array().map(Vec::len), Some(1));
        assert_eq!(doc["tags"][0]["name"], serde_json::json!("pets"));
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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

        let api = &manifest.apis[0];
        assert_eq!(api.name, "petstore");
        assert!(api.description.is_some());
        assert_eq!(api.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn test_spec_to_manifest_no_deprecated() {
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "petstore", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "advanced", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "advanced", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "advanced", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "advanced", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "advanced", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "advanced", None).unwrap();

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
        let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(&doc, "test", None).unwrap();

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
        let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(&doc, "test", None).unwrap();

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
        let yaml = r#"
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
"#;
        let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(&doc, "test", None).unwrap();

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
        let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(&doc, "test", None).unwrap();

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
        let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(&doc, "test", None).unwrap();

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
        let doc = load_spec_from_file(Path::new("testdata/advanced.yaml")).unwrap();
        let manifest = spec_to_manifest(&doc, "advanced", None).unwrap();

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

    #[test]
    fn test_inline_object_parsed_as_inline_object() {
        let spec_json = serde_json::json!({
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "servers": [{ "url": "https://api.example.com" }],
            "paths": {},
            "components": {
                "schemas": {
                    "Config": {
                        "type": "object",
                        "required": ["settings"],
                        "properties": {
                            "settings": {
                                "type": "object",
                                "required": ["timeout"],
                                "properties": {
                                    "timeout": { "type": "integer" },
                                    "retries": { "type": "integer" }
                                }
                            }
                        }
                    }
                }
            }
        });
        let doc: serde_json::Value = serde_json::from_value(spec_json).unwrap();
        let manifest = spec_to_manifest(&doc, "test", None).unwrap();
        let config_schema = manifest
            .schemas
            .iter()
            .find(|s| s.name == "Config")
            .unwrap();
        let settings_field = config_schema
            .fields
            .iter()
            .find(|f| f.name == "settings")
            .unwrap();
        match &settings_field.field_type {
            FieldType::InlineObject { fields } => {
                assert!(
                    fields.iter().any(|f| f.name == "timeout"),
                    "Missing timeout field"
                );
                assert!(
                    fields.iter().any(|f| f.name == "retries"),
                    "Missing retries field"
                );
                let timeout = fields.iter().find(|f| f.name == "timeout").unwrap();
                assert!(timeout.required, "timeout should be required");
                let retries = fields.iter().find(|f| f.name == "retries").unwrap();
                assert!(!retries.required, "retries should not be required");
            }
            other => panic!("Expected InlineObject, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_server_url_spec_to_manifest_integration() {
        // Simulate the petstore3 scenario: spec has relative server URL /api/v3,
        // loaded from a remote URL.
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Petstore
  version: "1.0.0"
servers:
  - url: /api/v3
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: ok
"#;
        let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
        let manifest = spec_to_manifest(
            &doc,
            "petstore",
            Some("https://petstore3.swagger.io/api/v3/openapi.json"),
        )
        .unwrap();
        assert_eq!(
            manifest.apis[0].base_url,
            "https://petstore3.swagger.io/api/v3"
        );
    }
}
