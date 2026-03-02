#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use toolscript::codegen::manifest::*;
use toolscript::runtime::executor::{ExecutorConfig, ScriptExecutor};
use toolscript::runtime::http::{AuthCredentialsMap, HttpHandler};
use toolscript::runtime::mcp_client::McpClientManager;
use toolscript::server::ToolScriptServer;
use toolscript::server::tools;

fn mcp_only_manifest() -> Manifest {
    Manifest {
        apis: vec![],
        functions: vec![],
        schemas: vec![],
        mcp_servers: vec![McpServerEntry {
            name: "filesystem".to_string(),
            description: Some("File system access".to_string()),
            tools: vec![
                McpToolDef {
                    name: "read_file".to_string(),
                    server: "filesystem".to_string(),
                    description: Some("Read a file from disk".to_string()),
                    params: vec![McpParamDef {
                        name: "path".to_string(),
                        luau_type: "string".to_string(),
                        required: true,
                        description: Some("File path to read".to_string()),
                        ..Default::default()
                    }],
                    schemas: vec![],
                    output_schemas: vec![],
                },
                McpToolDef {
                    name: "write_file".to_string(),
                    server: "filesystem".to_string(),
                    description: Some("Write content to a file".to_string()),
                    params: vec![
                        McpParamDef {
                            name: "path".to_string(),
                            luau_type: "string".to_string(),
                            required: true,
                            description: None,
                            ..Default::default()
                        },
                        McpParamDef {
                            name: "content".to_string(),
                            luau_type: "string".to_string(),
                            required: true,
                            description: None,
                            ..Default::default()
                        },
                    ],
                    schemas: vec![],
                    output_schemas: vec![],
                },
            ],
        }],
    }
}

fn mixed_manifest() -> Manifest {
    Manifest {
        apis: vec![ApiConfig {
            name: "petstore".to_string(),
            base_url: "https://petstore.example.com/v1".to_string(),
            description: Some("Pet store API".to_string()),
            version: Some("1.0.0".to_string()),
            auth: None,
        }],
        functions: vec![FunctionDef {
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
            response_schema: None,
        }],
        schemas: vec![],
        mcp_servers: vec![McpServerEntry {
            name: "filesystem".to_string(),
            description: Some("File system access".to_string()),
            tools: vec![McpToolDef {
                name: "read_file".to_string(),
                server: "filesystem".to_string(),
                description: Some("Read a file".to_string()),
                params: vec![McpParamDef {
                    name: "path".to_string(),
                    luau_type: "string".to_string(),
                    required: true,
                    description: Some("File path".to_string()),
                    ..Default::default()
                }],
                schemas: vec![],
                output_schemas: vec![],
            }],
        }],
    }
}

fn make_server(manifest: Manifest) -> ToolScriptServer {
    ToolScriptServer::new(
        manifest,
        Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
        AuthCredentialsMap::new(),
        ExecutorConfig::default(),
        None,
        Arc::new(McpClientManager::empty()),
    )
}

// ---- MCP-only mode tests ----

#[test]
fn test_mcp_only_list_apis() {
    let server = make_server(mcp_only_manifest());
    let result = tools::list_apis_impl(&server);
    let json: serde_json::Value = serde_json::from_str(&result).unwrap();
    let apis = json.as_array().unwrap();
    assert_eq!(apis.len(), 2); // 1 MCP + 1 luau builtin
    assert_eq!(apis[0]["name"], "filesystem");
    assert_eq!(apis[0]["source"], "mcp");
    assert_eq!(apis[1]["name"], "luau");
    assert_eq!(apis[1]["source"], "builtin");
}

#[test]
fn test_mcp_only_list_functions() {
    let server = make_server(mcp_only_manifest());
    let result = tools::list_functions_impl(&server, None, None);
    let json: serde_json::Value = serde_json::from_str(&result).unwrap();
    let funcs = json.as_array().unwrap();
    assert_eq!(funcs.len(), 6); // 2 MCP + 4 builtins (no io)
    // MCP tools should be from filesystem
    let mcp_funcs: Vec<_> = funcs.iter().filter(|f| f["source"] == "mcp").collect();
    assert_eq!(mcp_funcs.len(), 2);
    for f in &mcp_funcs {
        assert_eq!(f["api"], "filesystem");
    }
    // Builtins should be from luau
    let builtin_funcs: Vec<_> = funcs.iter().filter(|f| f["source"] == "builtin").collect();
    assert_eq!(builtin_funcs.len(), 4);
    for f in &builtin_funcs {
        assert_eq!(f["api"], "luau");
    }
}

#[test]
fn test_mcp_only_get_function_docs() {
    let server = make_server(mcp_only_manifest());
    let result = tools::get_function_docs_impl(&server, "filesystem.read_file");
    assert!(result.is_ok());
    let docs = result.unwrap();
    assert!(
        docs.contains("function sdk.filesystem.read_file"),
        "Missing function sig. Got:\n{docs}"
    );
    assert!(
        docs.contains("path: string"),
        "Missing path param. Got:\n{docs}"
    );
}

#[test]
fn test_mcp_only_search_docs() {
    let server = make_server(mcp_only_manifest());
    let result = tools::search_docs_impl(&server, "read");
    let json: serde_json::Value = serde_json::from_str(&result).unwrap();
    let items = json.as_array().unwrap();
    assert!(!items.is_empty());
    assert!(items.iter().any(|i| i["name"] == "filesystem.read_file"));
}

// ---- Mixed mode tests ----

#[test]
fn test_mixed_mode_all_visible() {
    let server = make_server(mixed_manifest());

    // All three sources should appear in list_apis
    let apis_json: serde_json::Value =
        serde_json::from_str(&tools::list_apis_impl(&server)).unwrap();
    let apis = apis_json.as_array().unwrap();
    assert_eq!(apis.len(), 3); // 1 OpenAPI + 1 MCP + 1 luau

    // All should appear in list_functions
    let funcs_json: serde_json::Value =
        serde_json::from_str(&tools::list_functions_impl(&server, None, None)).unwrap();
    let funcs = funcs_json.as_array().unwrap();
    assert_eq!(funcs.len(), 6); // 1 OpenAPI + 1 MCP + 4 builtins (no io)

    // All should be findable via get_function_docs
    assert!(tools::get_function_docs_impl(&server, "list_pets").is_ok());
    assert!(tools::get_function_docs_impl(&server, "filesystem.read_file").is_ok());
    assert!(tools::get_function_docs_impl(&server, "json.encode").is_ok());
}

// ---- MCP tool docs with schemas test ----

#[test]
fn test_mcp_tool_docs_with_schemas() {
    let manifest = Manifest {
        apis: vec![],
        functions: vec![],
        schemas: vec![],
        mcp_servers: vec![McpServerEntry {
            name: "users".to_string(),
            description: None,
            tools: vec![McpToolDef {
                name: "create_user".to_string(),
                server: "users".to_string(),
                description: Some("Create a new user".to_string()),
                params: vec![McpParamDef {
                    name: "data".to_string(),
                    luau_type: "UserInput".to_string(),
                    required: true,
                    description: None,
                    field_type: FieldType::Object {
                        schema: "UserInput".to_string(),
                    },
                }],
                schemas: vec![SchemaDef {
                    name: "UserInput".to_string(),
                    description: Some("User creation data".to_string()),
                    fields: vec![
                        FieldDef {
                            name: "name".to_string(),
                            field_type: FieldType::String,
                            required: true,
                            description: None,
                            enum_values: None,
                            nullable: false,
                            format: None,
                        },
                        FieldDef {
                            name: "email".to_string(),
                            field_type: FieldType::String,
                            required: false,
                            description: None,
                            enum_values: None,
                            nullable: false,
                            format: None,
                        },
                    ],
                }],
                output_schemas: vec![],
            }],
        }],
    };
    let server = make_server(manifest);
    let docs = tools::get_function_docs_impl(&server, "users.create_user").unwrap();
    assert!(
        docs.contains("function sdk.users.create_user"),
        "Missing function sig. Got:\n{docs}"
    );
    assert!(
        docs.contains("export type UserInput"),
        "Missing schema. Got:\n{docs}"
    );
    assert!(
        docs.contains("name: string"),
        "Missing name field. Got:\n{docs}"
    );
}

// ---- Script execution with MCP tools registered ----

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_sdk_table_structure() {
    // Verify that a script can see sdk.<server> namespace
    let manifest = mcp_only_manifest();
    let executor = ScriptExecutor::new(
        manifest,
        Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
        ExecutorConfig::default(),
        None,
        Arc::new(McpClientManager::empty()),
    );
    let auth = AuthCredentialsMap::new();

    // Script checks that sdk.filesystem exists and is a table
    let result = executor
        .execute(r"return type(sdk.filesystem)", &auth, None)
        .await
        .unwrap();
    assert_eq!(result.result, serde_json::json!("table"));

    // Script checks that sdk.filesystem.read_file is a function
    let result = executor
        .execute(r"return type(sdk.filesystem.read_file)", &auth, None)
        .await
        .unwrap();
    assert_eq!(result.result, serde_json::json!("function"));
}
