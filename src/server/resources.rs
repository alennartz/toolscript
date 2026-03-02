use std::collections::HashMap;
use std::fmt::Write;

use rmcp::model::{AnnotateAble, RawResource, ReadResourceResult, Resource, ResourceContents};

use super::builtins;
use crate::codegen::annotations::{
    render_function_annotation, render_mcp_tool_annotation, render_schema_annotation,
};
use crate::codegen::manifest::Manifest;

/// Build the list of MCP resources for all APIs in the manifest.
///
/// Resources follow the URI scheme:
/// - `sdk://{api_name}/overview`
/// - `sdk://{api_name}/functions`
/// - `sdk://{api_name}/schemas`
/// - `sdk://{api_name}/functions/{func_name}`
/// - `sdk://{api_name}/schemas/{schema_name}`
#[allow(clippy::too_many_lines)]
pub fn build_resource_list(manifest: &Manifest, io_enabled: bool) -> Vec<Resource> {
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

    // MCP server resources
    for mcp_server in &manifest.mcp_servers {
        resources.push(
            RawResource {
                uri: format!("sdk://{}/overview", mcp_server.name),
                name: format!("{} Overview", mcp_server.name),
                title: Some(format!("{} MCP Server Overview", mcp_server.name)),
                description: mcp_server.description.clone(),
                mime_type: Some("text/plain".to_string()),
                size: None,
                icons: None,
                meta: None,
            }
            .no_annotation(),
        );

        resources.push(
            RawResource {
                uri: format!("sdk://{}/functions", mcp_server.name),
                name: format!("{} Tools", mcp_server.name),
                title: Some(format!("All {} tools", mcp_server.name)),
                description: Some(format!(
                    "All tool signatures for the {} MCP server",
                    mcp_server.name
                )),
                mime_type: Some("text/plain".to_string()),
                size: None,
                icons: None,
                meta: None,
            }
            .no_annotation(),
        );
    }

    // Built-in Luau globals resources
    resources.push(
        RawResource {
            uri: "sdk://luau/overview".to_string(),
            name: "Luau Overview".to_string(),
            title: Some("Built-in Luau Runtime".to_string()),
            description: Some(builtins::LUAU_DESCRIPTION.to_string()),
            mime_type: Some("text/plain".to_string()),
            size: None,
            icons: None,
            meta: None,
        }
        .no_annotation(),
    );

    resources.push(
        RawResource {
            uri: "sdk://luau/functions".to_string(),
            name: "Luau Functions".to_string(),
            title: Some("Built-in Luau functions".to_string()),
            description: Some("All built-in function signatures".to_string()),
            mime_type: Some("text/plain".to_string()),
            size: None,
            icons: None,
            meta: None,
        }
        .no_annotation(),
    );

    // Individual builtin function resources
    for builtin in builtins::builtin_functions(io_enabled) {
        resources.push(
            RawResource {
                uri: format!("sdk://luau/functions/{}", builtin.name),
                name: builtin.name.to_string(),
                title: Some(builtin.summary.to_string()),
                description: Some(builtin.summary.to_string()),
                mime_type: Some("text/plain".to_string()),
                size: None,
                icons: None,
                meta: None,
            }
            .no_annotation(),
        );
    }

    resources
}

/// Read a resource by URI, returning its content.
#[allow(clippy::implicit_hasher)] // always used with default HashMap
#[allow(clippy::too_many_lines)]
pub fn read_resource(
    uri: &str,
    manifest: &Manifest,
    annotation_cache: &HashMap<String, String>,
    io_enabled: bool,
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
    let resource_type = parts.get(1).unwrap_or(&"");

    // Check if this is a built-in Luau resource
    if api_name == "luau" {
        return read_luau_resource(uri, &parts, resource_type, annotation_cache, io_enabled);
    }

    // Check if this is an MCP server
    if let Some(mcp_server) = manifest.mcp_servers.iter().find(|s| s.name == api_name) {
        return read_mcp_resource(uri, mcp_server, resource_type);
    }

    let api = manifest
        .apis
        .iter()
        .find(|a| a.name == api_name)
        .ok_or_else(|| {
            rmcp::ErrorData::invalid_params(format!("API '{api_name}' not found"), None)
        })?;

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
                let schema = manifest
                    .schemas
                    .iter()
                    .find(|s| s.name == *schema_name)
                    .ok_or_else(|| {
                        rmcp::ErrorData::invalid_params(
                            format!("Schema '{schema_name}' not found"),
                            None,
                        )
                    })?;
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(
                        render_schema_annotation(schema),
                        uri,
                    )],
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

/// Handle resource reads for built-in Luau URIs.
fn read_luau_resource(
    uri: &str,
    parts: &[&str],
    resource_type: &str,
    annotation_cache: &HashMap<String, String>,
    io_enabled: bool,
) -> Result<ReadResourceResult, rmcp::ErrorData> {
    match resource_type {
        "overview" => {
            let mut text = "# Luau Runtime\n\n".to_string();
            text.push_str(builtins::LUAU_DESCRIPTION);
            text.push_str("\n\nStandard Lua libraries are also available: string, table, math, os.clock(), os.date(), os.difftime(), os.time(). These follow standard Lua 5.1 behavior.\n");
            let count = builtins::builtin_functions(io_enabled).count();
            let _ = writeln!(text, "\nDocumented functions: {count}");
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
                // All functions
                let mut text = String::new();
                for builtin in builtins::builtin_functions(io_enabled) {
                    text.push_str(builtin.annotation);
                    text.push_str("\n\n");
                }
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(text, uri)],
                })
            }
        }
        _ => Err(rmcp::ErrorData::invalid_params(
            format!("Unknown resource: {uri}"),
            None,
        )),
    }
}

/// Handle resource reads for MCP server URIs.
fn read_mcp_resource(
    uri: &str,
    mcp_server: &crate::codegen::manifest::McpServerEntry,
    resource_type: &str,
) -> Result<ReadResourceResult, rmcp::ErrorData> {
    match resource_type {
        "overview" => {
            let mut text = format!("# {} MCP Server\n\n", mcp_server.name);
            if let Some(ref desc) = mcp_server.description {
                let _ = write!(text, "{desc}\n\n");
            }
            let _ = writeln!(text, "Tools: {}", mcp_server.tools.len());
            let _ = writeln!(text, "\nAvailable tools:");
            for tool in &mcp_server.tools {
                let desc = tool.description.as_deref().unwrap_or("No description");
                let _ = writeln!(text, "  - {}: {desc}", tool.name);
            }

            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(text, uri)],
            })
        }
        "functions" => {
            let mut text = String::new();
            for tool in &mcp_server.tools {
                text.push_str(&render_mcp_tool_annotation(tool));
                text.push_str("\n\n");
            }
            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(text, uri)],
            })
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
    use crate::codegen::annotations::render_function_annotation;
    use crate::server::tests::test_manifest;

    fn build_annotation_cache(manifest: &Manifest) -> HashMap<String, String> {
        let mut cache: HashMap<String, String> = manifest
            .functions
            .iter()
            .map(|f| (f.name.clone(), render_function_annotation(f)))
            .collect();
        // Include builtins in annotation cache for resource tests
        for builtin in builtins::builtin_functions(false) {
            cache.insert(builtin.name.to_string(), builtin.annotation.to_string());
        }
        cache
    }

    #[test]
    fn test_resource_list_contains_expected_uris() {
        let manifest = test_manifest();
        let resources = build_resource_list(&manifest, false);

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
    fn test_resource_list_contains_luau_uris() {
        let manifest = test_manifest();
        let resources = build_resource_list(&manifest, false);
        let uris: Vec<&str> = resources.iter().map(|r| r.uri.as_str()).collect();

        assert!(uris.contains(&"sdk://luau/overview"));
        assert!(uris.contains(&"sdk://luau/functions"));
        // Without io, should have json.encode, json.decode, print, os.clock
        assert!(uris.contains(&"sdk://luau/functions/json.encode"));
        assert!(uris.contains(&"sdk://luau/functions/print"));
        assert!(!uris.contains(&"sdk://luau/functions/io.open"));
    }

    #[test]
    fn test_resource_list_contains_luau_io_uris() {
        let manifest = test_manifest();
        let resources = build_resource_list(&manifest, true);
        let uris: Vec<&str> = resources.iter().map(|r| r.uri.as_str()).collect();

        assert!(uris.contains(&"sdk://luau/functions/io.open"));
        assert!(uris.contains(&"sdk://luau/functions/os.remove"));
    }

    #[test]
    fn test_read_overview_resource() {
        let manifest = test_manifest();
        let ac = build_annotation_cache(&manifest);
        let result = read_resource("sdk://petstore/overview", &manifest, &ac, false).unwrap();

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
    fn test_read_luau_overview_resource() {
        let manifest = test_manifest();
        let ac = build_annotation_cache(&manifest);
        let result = read_resource("sdk://luau/overview", &manifest, &ac, false).unwrap();

        assert_eq!(result.contents.len(), 1);
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            assert!(text.contains("Luau Runtime"), "Got:\n{text}");
            assert!(text.contains("Documented functions: 4"), "Got:\n{text}");
        } else {
            panic!("Expected TextResourceContents");
        }
    }

    #[test]
    fn test_read_luau_functions_resource() {
        let manifest = test_manifest();
        let ac = build_annotation_cache(&manifest);
        let result = read_resource("sdk://luau/functions", &manifest, &ac, false).unwrap();

        assert_eq!(result.contents.len(), 1);
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            assert!(text.contains("json.encode"), "Got:\n{text}");
            assert!(text.contains("json.decode"), "Got:\n{text}");
            assert!(text.contains("function print"), "Got:\n{text}");
        } else {
            panic!("Expected TextResourceContents");
        }
    }

    #[test]
    fn test_read_function_resource() {
        let manifest = test_manifest();
        let ac = build_annotation_cache(&manifest);
        let result =
            read_resource("sdk://petstore/functions/list_pets", &manifest, &ac, false).unwrap();

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
        let ac = build_annotation_cache(&manifest);
        let result = read_resource("sdk://petstore/schemas/Pet", &manifest, &ac, false).unwrap();

        assert_eq!(result.contents.len(), 1);
        if let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] {
            assert!(text.contains("export type Pet = {"));
            assert!(text.contains("id: string,"));
        } else {
            panic!("Expected TextResourceContents");
        }
    }
}
