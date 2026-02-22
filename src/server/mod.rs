pub mod resources;
pub mod tools;

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::ServerHandler;
use rmcp::handler::server::router::Router;
use rmcp::model::{
    Implementation, ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResult, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};

use crate::codegen::annotations::{render_function_annotation, render_schema_annotation};
use crate::codegen::manifest::Manifest;
use crate::runtime::executor::{ExecutorConfig, ScriptExecutor};
use crate::runtime::http::{AuthCredentialsMap, HttpHandler};

/// The MCP server struct that holds all state needed to serve documentation tools
/// and execute scripts.
pub struct CodeMcpServer {
    /// The manifest containing API configurations, functions, and schemas.
    pub manifest: Manifest,
    /// The script executor for running Lua scripts.
    pub executor: ScriptExecutor,
    /// Pre-rendered function annotations indexed by function name.
    pub annotation_cache: HashMap<String, String>,
    /// Pre-rendered schema annotations indexed by schema name.
    pub schema_cache: HashMap<String, String>,
    /// Authentication credentials loaded from environment.
    pub auth: AuthCredentialsMap,
}

impl CodeMcpServer {
    /// Create a new MCP server from a manifest and configuration.
    pub fn new(
        manifest: Manifest,
        handler: Arc<HttpHandler>,
        auth: AuthCredentialsMap,
        config: ExecutorConfig,
    ) -> Self {
        // Pre-render all annotations into caches
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

        let executor = ScriptExecutor::new(manifest.clone(), handler, config);

        Self {
            manifest,
            executor,
            annotation_cache,
            schema_cache,
            auth,
        }
    }

    /// Build the server info for the MCP protocol initialize response.
    fn server_info() -> ServerInfo {
        ServerInfo {
            protocol_version: rmcp::model::ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            server_info: Implementation {
                name: "code-mcp".to_string(),
                title: Some("code-mcp".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                description: Some(
                    "MCP server that generates Lua SDK from OpenAPI specs".to_string(),
                ),
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "Use list_apis and list_functions to discover available APIs, \
                 get_function_docs for detailed documentation, and execute_script \
                 to run Lua scripts against the SDK."
                    .to_string(),
            ),
        }
    }

    /// Build a Router that wires tools + the server handler together.
    pub fn into_router(self) -> Router<Self> {
        Router::new(self)
            .with_tool(tools::list_apis_tool())
            .with_tool(tools::list_functions_tool())
            .with_tool(tools::get_function_docs_tool())
            .with_tool(tools::search_docs_tool())
            .with_tool(tools::get_schema_tool())
            .with_tool(tools::execute_script_tool())
    }
}

impl ServerHandler for CodeMcpServer {
    fn get_info(&self) -> ServerInfo {
        Self::server_info()
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, rmcp::ErrorData>> + Send + '_
    {
        std::future::ready(Ok(ListResourcesResult {
            resources: resources::build_resource_list(&self.manifest),
            ..Default::default()
        }))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, rmcp::ErrorData>> + Send + '_
    {
        let result = resources::read_resource(
            &request.uri,
            &self.manifest,
            &self.annotation_cache,
            &self.schema_cache,
        );
        std::future::ready(result)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::codegen::manifest::*;
    use crate::runtime::http::HttpHandler;

    /// Create a test manifest with a petstore API.
    pub(crate) fn test_manifest() -> Manifest {
        Manifest {
            apis: vec![ApiConfig {
                name: "petstore".to_string(),
                base_url: "https://petstore.example.com/v1".to_string(),
                description: Some("A sample petstore API".to_string()),
                version: Some("1.0.0".to_string()),
                auth: Some(AuthConfig::Bearer {
                    header: "Authorization".to_string(),
                    prefix: "Bearer ".to_string(),
                }),
            }],
            functions: vec![
                FunctionDef {
                    name: "list_pets".to_string(),
                    api: "petstore".to_string(),
                    tag: Some("pets".to_string()),
                    method: HttpMethod::Get,
                    path: "/pets".to_string(),
                    summary: Some("List all pets".to_string()),
                    description: Some("Returns all pets from the store".to_string()),
                    deprecated: false,
                    parameters: vec![ParamDef {
                        name: "limit".to_string(),
                        location: ParamLocation::Query,
                        param_type: ParamType::Integer,
                        required: false,
                        description: Some("Max items to return".to_string()),
                        default: None,
                        enum_values: None,
                    }],
                    request_body: None,
                    response_schema: Some("Pet".to_string()),
                },
                FunctionDef {
                    name: "get_pet".to_string(),
                    api: "petstore".to_string(),
                    tag: Some("pets".to_string()),
                    method: HttpMethod::Get,
                    path: "/pets/{pet_id}".to_string(),
                    summary: Some("Get a pet by ID".to_string()),
                    description: None,
                    deprecated: false,
                    parameters: vec![ParamDef {
                        name: "pet_id".to_string(),
                        location: ParamLocation::Path,
                        param_type: ParamType::String,
                        required: true,
                        description: Some("The pet's ID".to_string()),
                        default: None,
                        enum_values: None,
                    }],
                    request_body: None,
                    response_schema: Some("Pet".to_string()),
                },
                FunctionDef {
                    name: "create_pet".to_string(),
                    api: "petstore".to_string(),
                    tag: Some("pets".to_string()),
                    method: HttpMethod::Post,
                    path: "/pets".to_string(),
                    summary: Some("Create a pet".to_string()),
                    description: None,
                    deprecated: true,
                    parameters: vec![],
                    request_body: Some(RequestBodyDef {
                        content_type: "application/json".to_string(),
                        schema: "NewPet".to_string(),
                        required: true,
                        description: None,
                    }),
                    response_schema: Some("Pet".to_string()),
                },
            ],
            schemas: vec![
                SchemaDef {
                    name: "Pet".to_string(),
                    description: Some("A pet in the store".to_string()),
                    fields: vec![
                        FieldDef {
                            name: "id".to_string(),
                            field_type: FieldType::String,
                            required: true,
                            description: Some("Unique identifier".to_string()),
                            enum_values: None,
                        },
                        FieldDef {
                            name: "name".to_string(),
                            field_type: FieldType::String,
                            required: true,
                            description: Some("Pet name".to_string()),
                            enum_values: None,
                        },
                    ],
                },
                SchemaDef {
                    name: "NewPet".to_string(),
                    description: Some("Data for creating a new pet".to_string()),
                    fields: vec![FieldDef {
                        name: "name".to_string(),
                        field_type: FieldType::String,
                        required: true,
                        description: Some("Pet name".to_string()),
                        enum_values: None,
                    }],
                },
            ],
        }
    }

    fn test_server() -> CodeMcpServer {
        CodeMcpServer::new(
            test_manifest(),
            Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
            AuthCredentialsMap::new(),
            ExecutorConfig::default(),
        )
    }

    #[test]
    fn test_list_apis() {
        let server = test_server();
        let result = tools::list_apis_impl(&server);
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        let apis = json.as_array().unwrap();
        assert_eq!(apis.len(), 1);
        assert_eq!(apis[0]["name"], "petstore");
        assert_eq!(apis[0]["base_url"], "https://petstore.example.com/v1");
        assert_eq!(apis[0]["version"], "1.0.0");
        assert_eq!(apis[0]["function_count"], 3);
    }

    #[test]
    fn test_list_functions_all() {
        let server = test_server();
        let result = tools::list_functions_impl(&server, None, None);
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        let funcs = json.as_array().unwrap();
        assert_eq!(funcs.len(), 3);
        // Check that create_pet has deprecated=true
        let create = funcs.iter().find(|f| f["name"] == "create_pet").unwrap();
        assert_eq!(create["deprecated"], true);
    }

    #[test]
    fn test_list_functions_filtered_by_tag() {
        let server = test_server();
        let result = tools::list_functions_impl(&server, None, Some("pets"));
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        let funcs = json.as_array().unwrap();
        assert_eq!(funcs.len(), 3); // all are tagged "pets"

        // Filter by non-existent tag
        let result = tools::list_functions_impl(&server, None, Some("users"));
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        let funcs = json.as_array().unwrap();
        assert_eq!(funcs.len(), 0);
    }

    #[test]
    fn test_get_function_docs_found() {
        let server = test_server();
        let result = tools::get_function_docs_impl(&server, "list_pets");
        assert!(result.is_ok());
        let docs = result.unwrap();
        assert!(docs.contains("List all pets"));
        assert!(docs.contains("@param limit?"));
        assert!(docs.contains("@return Pet"));
        assert!(docs.contains("function sdk.list_pets"));
    }

    #[test]
    fn test_get_function_docs_not_found() {
        let server = test_server();
        let result = tools::get_function_docs_impl(&server, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_search_docs() {
        let server = test_server();
        let results = tools::search_docs_impl(&server, "pet");
        let json: serde_json::Value = serde_json::from_str(&results).unwrap();
        let items = json.as_array().unwrap();
        // Should find functions and schemas that mention "pet"
        assert!(!items.is_empty());
    }

    #[test]
    fn test_get_schema_found() {
        let server = test_server();
        let result = tools::get_schema_impl(&server, "Pet");
        assert!(result.is_ok());
        let docs = result.unwrap();
        assert!(docs.contains("@class Pet"));
        assert!(docs.contains("@field id string"));
        assert!(docs.contains("@field name string"));
    }

    #[test]
    fn test_get_schema_not_found() {
        let server = test_server();
        let result = tools::get_schema_impl(&server, "Nonexistent");
        assert!(result.is_err());
    }
}
