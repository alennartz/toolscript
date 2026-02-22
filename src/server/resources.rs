use std::collections::HashMap;
use std::fmt::Write;

use rmcp::model::{AnnotateAble, RawResource, ReadResourceResult, Resource, ResourceContents};

use crate::codegen::annotations::{render_function_annotation, render_schema_annotation};
use crate::codegen::manifest::Manifest;

/// Build the list of MCP resources for all APIs in the manifest.
///
/// Resources follow the URI scheme:
/// - `sdk://{api_name}/overview`
/// - `sdk://{api_name}/functions`
/// - `sdk://{api_name}/schemas`
/// - `sdk://{api_name}/functions/{func_name}`
/// - `sdk://{api_name}/schemas/{schema_name}`
pub fn build_resource_list(manifest: &Manifest) -> Vec<Resource> {
    let mut resources = Vec::new();

    for api in &manifest.apis {
        // Overview resource
        resources.push(
            RawResource {
                uri: format!("sdk://{}/overview", api.name),
                name: format!("{} Overview", api.name),
                title: Some(format!("{} API Overview", api.name)),
                description: api.description.clone(),
                mime_type: Some("text/plain".to_string()),
                size: None,
                icons: None,
                meta: None,
            }
            .no_annotation(),
        );

        // Functions listing resource
        resources.push(
            RawResource {
                uri: format!("sdk://{}/functions", api.name),
                name: format!("{} Functions", api.name),
                title: Some(format!("All {} functions", api.name)),
                description: Some(format!("All function signatures for the {} API", api.name)),
                mime_type: Some("text/plain".to_string()),
                size: None,
                icons: None,
                meta: None,
            }
            .no_annotation(),
        );

        // Schemas listing resource
        resources.push(
            RawResource {
                uri: format!("sdk://{}/schemas", api.name),
                name: format!("{} Schemas", api.name),
                title: Some(format!("All {} schemas", api.name)),
                description: Some(format!("All schema definitions for the {} API", api.name)),
                mime_type: Some("text/plain".to_string()),
                size: None,
                icons: None,
                meta: None,
            }
            .no_annotation(),
        );

        // Individual function resources
        for func in manifest.functions.iter().filter(|f| f.api == api.name) {
            resources.push(
                RawResource {
                    uri: format!("sdk://{}/functions/{}", api.name, func.name),
                    name: func.name.clone(),
                    title: func.summary.clone(),
                    description: func.summary.clone(),
                    mime_type: Some("text/plain".to_string()),
                    size: None,
                    icons: None,
                    meta: None,
                }
                .no_annotation(),
            );
        }

        // Individual schema resources
        for schema in &manifest.schemas {
            resources.push(
                RawResource {
                    uri: format!("sdk://{}/schemas/{}", api.name, schema.name),
                    name: schema.name.clone(),
                    title: Some(schema.name.clone()),
                    description: schema.description.clone(),
                    mime_type: Some("text/plain".to_string()),
                    size: None,
                    icons: None,
                    meta: None,
                }
                .no_annotation(),
            );
        }
    }

    resources
}

/// Read a resource by URI, returning its content.
#[allow(clippy::implicit_hasher)] // always used with default HashMap
pub fn read_resource(
    uri: &str,
    manifest: &Manifest,
    annotation_cache: &HashMap<String, String>,
    schema_cache: &HashMap<String, String>,
) -> Result<ReadResourceResult, rmcp::ErrorData> {
    // Parse the URI: sdk://{api_name}/{path...}
    let stripped = uri
        .strip_prefix("sdk://")
        .ok_or_else(|| rmcp::ErrorData::invalid_params("Invalid resource URI scheme", None))?;

    let parts: Vec<&str> = stripped.splitn(3, '/').collect();
    if parts.is_empty() {
        return Err(rmcp::ErrorData::invalid_params(
            "Invalid resource URI",
            None,
        ));
    }

    let api_name = parts[0];
    let api = manifest
        .apis
        .iter()
        .find(|a| a.name == api_name)
        .ok_or_else(|| {
            rmcp::ErrorData::invalid_params(format!("API '{api_name}' not found"), None)
        })?;

    let resource_type = parts.get(1).unwrap_or(&"");

    match *resource_type {
        "overview" => {
            let mut text = format!("# {} API\n\n", api.name);
            if let Some(ref desc) = api.description {
                let _ = write!(text, "{desc}\n\n");
            }
            if let Some(ref version) = api.version {
                let _ = writeln!(text, "Version: {version}");
            }
            let _ = writeln!(text, "Base URL: {}", api.base_url);

            let func_count = manifest
                .functions
                .iter()
                .filter(|f| f.api == api_name)
                .count();
            let _ = writeln!(text, "Functions: {func_count}");

            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(text, uri)],
            })
        }
        "functions" => {
            if let Some(func_name) = parts.get(2) {
                // Individual function
                let annotation = annotation_cache.get(*func_name).ok_or_else(|| {
                    rmcp::ErrorData::invalid_params(
                        format!("Function '{func_name}' not found"),
                        None,
                    )
                })?;
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(annotation.clone(), uri)],
                })
            } else {
                // All functions for this API
                let mut text = String::new();
                for func in manifest.functions.iter().filter(|f| f.api == api_name) {
                    text.push_str(&render_function_annotation(func));
                    text.push_str("\n\n");
                }
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(text, uri)],
                })
            }
        }
        "schemas" => {
            if let Some(schema_name) = parts.get(2) {
                // Individual schema
                let annotation = schema_cache.get(*schema_name).ok_or_else(|| {
                    rmcp::ErrorData::invalid_params(
                        format!("Schema '{schema_name}' not found"),
                        None,
                    )
                })?;
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(annotation.clone(), uri)],
                })
            } else {
                // All schemas
                let mut text = String::new();
                for schema in &manifest.schemas {
                    text.push_str(&render_schema_annotation(schema));
                    text.push_str("\n\n");
                }
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(text, uri)],
                })
            }
        }
        _ => Err(rmcp::ErrorData::invalid_params(
            format!("Unknown resource path: {uri}"),
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::codegen::annotations::{render_function_annotation, render_schema_annotation};
    use crate::server::tests::test_manifest;

    fn build_caches(manifest: &Manifest) -> (HashMap<String, String>, HashMap<String, String>) {
        let annotation_cache: HashMap<String, String> = manifest
            .functions
            .iter()
            .map(|f| (f.name.clone(), render_function_annotation(f)))
            .collect();
        let schema_cache: HashMap<String, String> = manifest
            .schemas
            .iter()
            .map(|s| (s.name.clone(), render_schema_annotation(s)))
            .collect();
        (annotation_cache, schema_cache)
    }

    #[test]
    fn test_resource_list_contains_expected_uris() {
        let manifest = test_manifest();
        let resources = build_resource_list(&manifest);

        let uris: Vec<&str> = resources.iter().map(|r| r.uri.as_str()).collect();

        // Check overview, functions, schemas listings exist
        assert!(uris.contains(&"sdk://petstore/overview"));
        assert!(uris.contains(&"sdk://petstore/functions"));
        assert!(uris.contains(&"sdk://petstore/schemas"));

        // Check individual function resources
        assert!(uris.contains(&"sdk://petstore/functions/list_pets"));
        assert!(uris.contains(&"sdk://petstore/functions/get_pet"));
        assert!(uris.contains(&"sdk://petstore/functions/create_pet"));

        // Check individual schema resources
        assert!(uris.contains(&"sdk://petstore/schemas/Pet"));
        assert!(uris.contains(&"sdk://petstore/schemas/NewPet"));
    }

    #[test]
    fn test_read_overview_resource() {
        let manifest = test_manifest();
        let (ac, sc) = build_caches(&manifest);
        let result = read_resource("sdk://petstore/overview", &manifest, &ac, &sc).unwrap();

        assert_eq!(result.contents.len(), 1);
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            assert!(text.contains("petstore"));
            assert!(text.contains("1.0.0"));
            assert!(text.contains("https://petstore.example.com/v1"));
        } else {
            panic!("Expected TextResourceContents");
        }
    }

    #[test]
    fn test_read_function_resource() {
        let manifest = test_manifest();
        let (ac, sc) = build_caches(&manifest);
        let result =
            read_resource("sdk://petstore/functions/list_pets", &manifest, &ac, &sc).unwrap();

        assert_eq!(result.contents.len(), 1);
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            assert!(text.contains("List all pets"));
            assert!(text.contains("function sdk.list_pets"));
        } else {
            panic!("Expected TextResourceContents");
        }
    }

    #[test]
    fn test_read_schema_resource() {
        let manifest = test_manifest();
        let (ac, sc) = build_caches(&manifest);
        let result = read_resource("sdk://petstore/schemas/Pet", &manifest, &ac, &sc).unwrap();

        assert_eq!(result.contents.len(), 1);
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            assert!(text.contains("export type Pet = {"));
            assert!(text.contains("id: string,"));
        } else {
            panic!("Expected TextResourceContents");
        }
    }
}
