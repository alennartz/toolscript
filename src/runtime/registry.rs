use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mlua::{LuaSerdeExt, MultiValue, Value};

use crate::codegen::manifest::{Manifest, ParamLocation, ParamType};
use crate::runtime::http::{AuthCredentials, AuthCredentialsMap, HttpHandler};
use crate::runtime::mcp_client::McpClientManager;
use crate::runtime::sandbox::Sandbox;
use crate::runtime::validate;

/// Register all manifest functions into the sandbox's `sdk` table.
///
/// Each `FunctionDef` becomes a Lua function under `sdk.<name>` that:
/// 1. Validates required parameters
/// 2. Builds the URL with path param substitution
/// 3. Collects query parameters
/// 4. Serializes request body
/// 5. Makes the HTTP call
/// 6. Returns the response as a Lua table
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub fn register_functions(
    sandbox: &Sandbox,
    manifest: &Manifest,
    handler: Arc<HttpHandler>,
    credentials: Arc<AuthCredentialsMap>,
    api_call_counter: Arc<AtomicUsize>,
    max_api_calls: Option<usize>,
) -> anyhow::Result<()> {
    let lua = sandbox.lua();
    let sdk: mlua::Table = lua.globals().get("sdk")?;

    // Build a lookup from API name -> (base_url, auth_config)
    let api_lookup: std::collections::HashMap<
        String,
        (&str, Option<&crate::codegen::manifest::AuthConfig>),
    > = manifest
        .apis
        .iter()
        .map(|api| (api.name.clone(), (api.base_url.as_str(), api.auth.as_ref())))
        .collect();

    for func_def in &manifest.functions {
        let func_name = func_def.name.clone();
        let (base_url, auth_config) = api_lookup.get(&func_def.api).ok_or_else(|| {
            anyhow::anyhow!(
                "function '{}' references unknown API '{}'",
                func_name,
                func_def.api
            )
        })?;

        let base_url = base_url.to_string();
        let auth_config_owned = auth_config.cloned();
        let func_def_clone = func_def.clone();
        let handler_clone = Arc::clone(&handler);
        let credentials_clone = Arc::clone(&credentials);
        let counter_clone = Arc::clone(&api_call_counter);
        let max_calls = max_api_calls;

        let lua_fn = lua.create_function(move |lua, args: MultiValue| {
            let func_def = &func_def_clone;
            let handler = &handler_clone;
            let credentials = &credentials_clone;
            let counter = &counter_clone;

            // Check API call limit
            let current_count = counter.load(Ordering::SeqCst);
            if let Some(max) = max_calls
                && current_count >= max
            {
                return Err(mlua::Error::external(anyhow::anyhow!(
                    "API call limit exceeded (max {max} calls)",
                )));
            }

            let arg_values: Vec<Value> = args.into_iter().collect();

            // Determine calling convention
            let has_visible_params = func_def.parameters.iter().any(|p| p.frozen_value.is_none());
            let has_body = func_def.request_body.is_some();

            // Extract params table based on calling convention
            let params_table: Option<mlua::Table> = if has_visible_params {
                match arg_values.first().cloned().unwrap_or(Value::Nil) {
                    Value::Table(t) => Some(t),
                    Value::Nil => None,
                    other => {
                        return Err(mlua::Error::external(anyhow::anyhow!(
                            "expected table as first argument to '{}', got {}",
                            func_def.name,
                            other.type_name()
                        )));
                    }
                }
            } else {
                None
            };

            let body_arg_idx = usize::from(has_visible_params);

            // Build path, query, and header params
            let mut url = base_url.clone();
            let mut path = func_def.path.clone();
            let mut query_params: Vec<(String, String)> = Vec::new();
            let mut header_params: Vec<(String, String)> = Vec::new();

            for param in &func_def.parameters {
                let str_value = if let Some(ref frozen) = param.frozen_value {
                    // Frozen param — use configured value directly, skip validation
                    frozen.clone()
                } else {
                    // Non-frozen — extract from table by name
                    let value: Value = params_table
                        .as_ref()
                        .map(|t| t.get::<Value>(param.name.as_str()))
                        .transpose()?
                        .unwrap_or(Value::Nil);

                    if param.required && matches!(value, Value::Nil) {
                        return Err(mlua::Error::external(anyhow::anyhow!(
                            "missing required parameter '{}' for function '{}'",
                            param.name,
                            func_def.name
                        )));
                    }

                    if matches!(value, Value::Nil) {
                        continue;
                    }

                    let str_val = match (&param.param_type, &value) {
                        #[allow(clippy::cast_possible_truncation)]
                        (ParamType::Integer, Value::Number(n)) => {
                            format!("{}", n.round() as i64)
                        }
                        _ => lua_value_to_string(&value),
                    };

                    // Validate enum and format constraints
                    validate::validate_param_value(&func_def.name, param, &str_val)?;

                    str_val
                };

                match param.location {
                    ParamLocation::Path => {
                        path = path.replace(&format!("{{{}}}", param.name), &str_value);
                    }
                    ParamLocation::Query => {
                        query_params.push((param.name.clone(), str_value));
                    }
                    ParamLocation::Header => {
                        header_params.push((param.name.clone(), str_value));
                    }
                }
            }

            url.push_str(&path);

            // Extract request body
            let body: Option<serde_json::Value> = if has_body {
                if body_arg_idx < arg_values.len() {
                    let body_val = arg_values[body_arg_idx].clone();
                    if matches!(body_val, Value::Nil) {
                        None
                    } else {
                        let json_body: serde_json::Value =
                            lua.from_value(body_val).map_err(|e| {
                                mlua::Error::external(anyhow::anyhow!(
                                    "failed to serialize request body: {e}",
                                ))
                            })?;
                        Some(json_body)
                    }
                } else {
                    None
                }
            } else {
                None
            };

            // Get method string
            let method = match func_def.method {
                crate::codegen::manifest::HttpMethod::Get => "GET",
                crate::codegen::manifest::HttpMethod::Post => "POST",
                crate::codegen::manifest::HttpMethod::Put => "PUT",
                crate::codegen::manifest::HttpMethod::Patch => "PATCH",
                crate::codegen::manifest::HttpMethod::Delete => "DELETE",
            };

            // Get credentials for this API
            let api_creds = credentials
                .get(&func_def.api)
                .cloned()
                .unwrap_or(AuthCredentials::None);

            // Increment API call counter
            counter.fetch_add(1, Ordering::SeqCst);

            // Make the HTTP call (blocking from Lua's perspective)
            let response = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(handler.request(
                    method,
                    &url,
                    auth_config_owned.as_ref(),
                    &api_creds,
                    &query_params,
                    &header_params,
                    body.as_ref(),
                ))
            })
            .map_err(mlua::Error::external)?;

            // Convert JSON response to Lua value
            let lua_value = lua.to_value(&response).map_err(|e| {
                mlua::Error::external(anyhow::anyhow!("failed to convert response to Lua: {e}"))
            })?;

            Ok(lua_value)
        })?;

        sdk.set(func_def.name.as_str(), lua_fn)?;
    }

    Ok(())
}

/// Convert a Lua value to a string for URL parameter encoding.
fn lua_value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_string_lossy(),
        Value::Integer(n) => n.to_string(),
        #[allow(clippy::cast_possible_truncation)]
        Value::Number(n) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        Value::Boolean(b) => b.to_string(),
        _ => String::new(),
    }
}

/// Register MCP tools into the sandbox as `sdk.<server>.<tool>()` closures.
///
/// Each MCP tool becomes a Lua function under `sdk.<server_name>.<tool_name>` that:
/// 1. Extracts the params table argument
/// 2. Converts Lua table to JSON object
/// 3. Calls `McpClientManager::call_tool` via `block_in_place`
/// 4. Converts the result content back to a Lua value
#[allow(clippy::needless_pass_by_value)]
pub fn register_mcp_tools(
    sandbox: &Sandbox,
    manifest: &Manifest,
    mcp_client: Arc<McpClientManager>,
    api_call_counter: Arc<AtomicUsize>,
    max_api_calls: Option<usize>,
) -> anyhow::Result<()> {
    let lua = sandbox.lua();
    let sdk: mlua::Table = lua.globals().get("sdk")?;

    for server in &manifest.mcp_servers {
        let server_table: mlua::Table = lua.create_table()?;

        for tool in &server.tools {
            let server_name = server.name.clone();
            let tool_name = tool.name.clone();
            let client = Arc::clone(&mcp_client);
            let counter = Arc::clone(&api_call_counter);
            let max = max_api_calls;

            let lua_fn = lua.create_function(move |lua, args: MultiValue| {
                // Check API call limit
                let current = counter.load(Ordering::SeqCst);
                if let Some(max) = max
                    && current >= max
                {
                    return Err(mlua::Error::external(anyhow::anyhow!(
                        "API call limit exceeded (max {max} calls)",
                    )));
                }

                // Extract params table -> JSON
                let arguments: Option<serde_json::Map<String, serde_json::Value>> =
                    match args.into_iter().next() {
                        Some(Value::Table(t)) => {
                            let json: serde_json::Value =
                                lua.from_value(Value::Table(t)).map_err(|e| {
                                    mlua::Error::external(anyhow::anyhow!(
                                        "failed to serialize arguments: {e}"
                                    ))
                                })?;
                            json.as_object().cloned()
                        }
                        Some(Value::Nil) | None => None,
                        Some(other) => {
                            return Err(mlua::Error::external(anyhow::anyhow!(
                                "expected table as argument to '{}.{}', got {}",
                                server_name,
                                tool_name,
                                other.type_name()
                            )));
                        }
                    };

                // Increment counter
                counter.fetch_add(1, Ordering::SeqCst);

                // Call the MCP tool
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(client.call_tool(
                        &server_name,
                        &tool_name,
                        arguments,
                    ))
                })
                .map_err(mlua::Error::external)?;

                // Convert result content to Lua
                convert_call_tool_result(lua, &result)
            })?;

            server_table.set(tool.name.as_str(), lua_fn)?;
        }

        sdk.set(server.name.as_str(), server_table)?;
    }

    Ok(())
}

/// Convert an MCP `CallToolResult` to a Lua value.
///
/// If the result contains `structured_content`, use that directly.
/// Otherwise, extract text from the content blocks:
/// - Single text that parses as JSON -> return as Lua table
/// - Single text that doesn't parse -> return as string
/// - No text content -> return nil
/// - Multiple text items -> return as array of strings
fn convert_call_tool_result(
    lua: &mlua::Lua,
    result: &rmcp::model::CallToolResult,
) -> Result<Value, mlua::Error> {
    // Check for MCP-level errors
    if result.is_error == Some(true) {
        let msg = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(mlua::Error::external(anyhow::anyhow!(
            "MCP tool error: {msg}"
        )));
    }

    // Prefer structured_content if present
    if let Some(ref structured) = result.structured_content {
        return lua.to_value(structured).map_err(|e| {
            mlua::Error::external(anyhow::anyhow!(
                "failed to convert MCP structured result: {e}"
            ))
        });
    }

    // Extract text content from the result
    // Content = Annotated<RawContent>, which Derefs to RawContent.
    // RawContent::Text(RawTextContent { text, .. })
    let texts: Vec<&str> = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect();

    if texts.len() == 1 {
        // Return as string — user can json.decode() if needed
        Ok(Value::String(lua.create_string(texts[0])?))
    } else if texts.is_empty() {
        Ok(Value::Nil)
    } else {
        // Multiple text items — return as array of strings
        let table = lua.create_table()?;
        for (i, text) in texts.iter().enumerate() {
            table.set(i + 1, *text)?;
        }
        Ok(Value::Table(table))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::codegen::manifest::*;
    use crate::runtime::sandbox::SandboxConfig;
    use std::sync::Mutex;

    fn test_manifest() -> Manifest {
        Manifest {
            apis: vec![ApiConfig {
                name: "petstore".to_string(),
                base_url: "https://petstore.example.com/v1".to_string(),
                description: None,
                version: None,
                auth: Some(AuthConfig::Bearer {
                    header: "Authorization".to_string(),
                    prefix: "Bearer ".to_string(),
                }),
            }],
            functions: vec![
                FunctionDef {
                    name: "get_pet".to_string(),
                    api: "petstore".to_string(),
                    tag: None,
                    method: HttpMethod::Get,
                    path: "/pets/{pet_id}".to_string(),
                    summary: None,
                    description: None,
                    deprecated: false,
                    parameters: vec![ParamDef {
                        name: "pet_id".to_string(),
                        location: ParamLocation::Path,
                        param_type: ParamType::String,
                        required: true,
                        description: None,
                        default: None,
                        enum_values: None,
                        format: None,
                        frozen_value: None,
                    }],
                    request_body: None,
                    response_schema: None,
                },
                FunctionDef {
                    name: "list_pets".to_string(),
                    api: "petstore".to_string(),
                    tag: None,
                    method: HttpMethod::Get,
                    path: "/pets".to_string(),
                    summary: None,
                    description: None,
                    deprecated: false,
                    parameters: vec![
                        ParamDef {
                            name: "status".to_string(),
                            location: ParamLocation::Query,
                            param_type: ParamType::String,
                            required: false,
                            description: None,
                            default: None,
                            enum_values: None,
                            format: None,
                            frozen_value: None,
                        },
                        ParamDef {
                            name: "limit".to_string(),
                            location: ParamLocation::Query,
                            param_type: ParamType::Integer,
                            required: false,
                            description: None,
                            default: None,
                            enum_values: None,
                            format: None,
                            frozen_value: None,
                        },
                    ],
                    request_body: None,
                    response_schema: None,
                },
                FunctionDef {
                    name: "create_pet".to_string(),
                    api: "petstore".to_string(),
                    tag: None,
                    method: HttpMethod::Post,
                    path: "/pets".to_string(),
                    summary: None,
                    description: None,
                    deprecated: false,
                    parameters: vec![],
                    request_body: Some(RequestBodyDef {
                        content_type: "application/json".to_string(),
                        schema: "Pet".to_string(),
                        required: true,
                        description: None,
                    }),
                    response_schema: None,
                },
            ],
            schemas: vec![],
            mcp_servers: vec![],
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_register_and_call_function() {
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = test_manifest();
        let handler = Arc::new(HttpHandler::mock(|_method, _url, _query, _body| {
            Ok(serde_json::json!({"id": "123", "name": "Fido", "status": "available"}))
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        let result: String = sb
            .eval(
                r#"
            local pet = sdk.get_pet({ pet_id = "123" })
            return pet.name
        "#,
            )
            .unwrap();
        assert_eq!(result, "Fido");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_path_param_substitution() {
        let captured_url = Arc::new(Mutex::new(String::new()));
        let captured_url_clone = Arc::clone(&captured_url);

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = test_manifest();
        let handler = Arc::new(HttpHandler::mock(move |_method, url, _query, _body| {
            *captured_url_clone.lock().unwrap() = url.to_string();
            Ok(serde_json::json!({"id": "456"}))
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        sb.eval::<Value>(r#"sdk.get_pet({ pet_id = "456" })"#)
            .unwrap();

        let url = captured_url.lock().unwrap().clone();
        assert_eq!(url, "https://petstore.example.com/v1/pets/456");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_query_params_passed() {
        let captured_query = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let captured_query_clone = Arc::clone(&captured_query);

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = test_manifest();
        let handler = Arc::new(HttpHandler::mock(move |_method, _url, query, _body| {
            *captured_query_clone.lock().unwrap() = query.to_vec();
            Ok(serde_json::json!([]))
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        sb.eval::<Value>(r#"sdk.list_pets({ status = "available", limit = 10 })"#)
            .unwrap();

        let query = captured_query.lock().unwrap().clone();
        assert_eq!(query.len(), 2);
        assert!(query.iter().any(|(k, v)| k == "status" && v == "available"));
        assert!(query.iter().any(|(k, v)| k == "limit" && v == "10"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_missing_required_param_errors() {
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = test_manifest();
        let handler = Arc::new(HttpHandler::mock(|_method, _url, _query, _body| {
            Ok(serde_json::json!({}))
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        let result = sb.eval::<Value>("sdk.get_pet()");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing required parameter"),
            "error was: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_optional_param_can_be_nil() {
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = test_manifest();
        let handler = Arc::new(HttpHandler::mock(|_method, _url, _query, _body| {
            Ok(serde_json::json!([]))
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        // Call with no arguments — both params are optional
        let result = sb.eval::<Value>("sdk.list_pets()");
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_request_body_sent() {
        let captured_body = Arc::new(Mutex::new(None::<serde_json::Value>));
        let captured_body_clone = Arc::clone(&captured_body);

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = test_manifest();
        let handler = Arc::new(HttpHandler::mock(move |_method, _url, _query, body| {
            *captured_body_clone.lock().unwrap() = body.cloned();
            Ok(serde_json::json!({"id": "new-1", "name": "Buddy"}))
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        sb.eval::<Value>(
            r#"
            sdk.create_pet({name = "Buddy", status = "available"})
        "#,
        )
        .unwrap();

        let body = captured_body.lock().unwrap().clone();
        assert!(body.is_some());
        let body = body.unwrap();
        assert_eq!(body["name"], "Buddy");
        assert_eq!(body["status"], "available");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_optional_header_param_omitted() {
        let captured_headers = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let captured_headers_clone = Arc::clone(&captured_headers);

        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "testapi".to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![FunctionDef {
                name: "get_thing".to_string(),
                api: "testapi".to_string(),
                tag: None,
                method: HttpMethod::Get,
                path: "/things/{id}".to_string(),
                summary: None,
                description: None,
                deprecated: false,
                parameters: vec![
                    ParamDef {
                        name: "id".to_string(),
                        location: ParamLocation::Path,
                        param_type: ParamType::String,
                        required: true,
                        description: None,
                        default: None,
                        enum_values: None,
                        format: None,
                        frozen_value: None,
                    },
                    ParamDef {
                        name: "X-Trace-ID".to_string(),
                        location: ParamLocation::Header,
                        param_type: ParamType::String,
                        required: false,
                        description: None,
                        default: None,
                        enum_values: None,
                        format: None,
                        frozen_value: None,
                    },
                ],
                request_body: None,
                response_schema: None,
            }],
            schemas: vec![],
            mcp_servers: vec![],
        };

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let handler = Arc::new(HttpHandler::mock_with_headers(
            move |_method, _url, _query, headers, _body| {
                *captured_headers_clone.lock().unwrap() = headers.to_vec();
                Ok(serde_json::json!({"ok": true}))
            },
        ));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        // Call with only the required path param, omit the optional header
        let result = sb.eval::<Value>(r#"sdk.get_thing({ id = "abc-123" })"#);
        assert!(
            result.is_ok(),
            "Call should succeed without optional header"
        );

        let headers = captured_headers.lock().unwrap().clone();
        assert!(
            !headers.iter().any(|(k, _)| k == "X-Trace-ID"),
            "Optional header should NOT be present when omitted. Got: {headers:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_header_param_integer_serialization() {
        let captured_headers = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let captured_headers_clone = Arc::clone(&captured_headers);

        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "testapi".to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![FunctionDef {
                name: "do_thing".to_string(),
                api: "testapi".to_string(),
                tag: None,
                method: HttpMethod::Get,
                path: "/things".to_string(),
                summary: None,
                description: None,
                deprecated: false,
                parameters: vec![ParamDef {
                    name: "X-Page-Size".to_string(),
                    location: ParamLocation::Header,
                    param_type: ParamType::Integer,
                    required: true,
                    description: None,
                    default: None,
                    enum_values: None,
                    format: None,
                    frozen_value: None,
                }],
                request_body: None,
                response_schema: None,
            }],
            schemas: vec![],
            mcp_servers: vec![],
        };

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let handler = Arc::new(HttpHandler::mock_with_headers(
            move |_method, _url, _query, headers, _body| {
                *captured_headers_clone.lock().unwrap() = headers.to_vec();
                Ok(serde_json::json!({"ok": true}))
            },
        ));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        // Pass a number from Lua
        sb.eval::<Value>(r#"sdk.do_thing({ ["X-Page-Size"] = 50 })"#)
            .unwrap();

        let headers = captured_headers.lock().unwrap().clone();
        let page_size = headers
            .iter()
            .find(|(k, _)| k == "X-Page-Size")
            .expect("X-Page-Size header should be present");
        assert_eq!(
            page_size.1, "50",
            "Integer header param should serialize to string '50'. Got: '{}'",
            page_size.1
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_header_params_sent() {
        let captured_headers = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let captured_headers_clone = Arc::clone(&captured_headers);

        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "testapi".to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![FunctionDef {
                name: "do_thing".to_string(),
                api: "testapi".to_string(),
                tag: None,
                method: HttpMethod::Get,
                path: "/things".to_string(),
                summary: None,
                description: None,
                deprecated: false,
                parameters: vec![
                    ParamDef {
                        name: "X-Request-ID".to_string(),
                        location: ParamLocation::Header,
                        param_type: ParamType::String,
                        required: true,
                        description: None,
                        default: None,
                        enum_values: None,
                        format: None,
                        frozen_value: None,
                    },
                    ParamDef {
                        name: "limit".to_string(),
                        location: ParamLocation::Query,
                        param_type: ParamType::Integer,
                        required: false,
                        description: None,
                        default: None,
                        enum_values: None,
                        format: None,
                        frozen_value: None,
                    },
                ],
                request_body: None,
                response_schema: None,
            }],
            schemas: vec![],
            mcp_servers: vec![],
        };

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let handler = Arc::new(HttpHandler::mock_with_headers(
            move |_method, _url, _query, headers, _body| {
                *captured_headers_clone.lock().unwrap() = headers.to_vec();
                Ok(serde_json::json!({"ok": true}))
            },
        ));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        sb.eval::<Value>(r#"sdk.do_thing({ ["X-Request-ID"] = "trace-123", limit = 10 })"#)
            .unwrap();

        let headers = captured_headers.lock().unwrap().clone();
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "X-Request-ID" && v == "trace-123"),
            "Header param not sent. Got: {headers:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_enum_validation_rejects_invalid() {
        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "testapi".to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![FunctionDef {
                name: "list_items".to_string(),
                api: "testapi".to_string(),
                tag: None,
                method: HttpMethod::Get,
                path: "/items".to_string(),
                summary: None,
                description: None,
                deprecated: false,
                parameters: vec![ParamDef {
                    name: "status".to_string(),
                    location: ParamLocation::Query,
                    param_type: ParamType::String,
                    required: true,
                    description: None,
                    default: None,
                    enum_values: Some(vec!["active".into(), "inactive".into()]),
                    format: None,
                    frozen_value: None,
                }],
                request_body: None,
                response_schema: None,
            }],
            schemas: vec![],
            mcp_servers: vec![],
        };

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let handler = Arc::new(HttpHandler::mock(|_method, _url, _query, _body| {
            panic!("HTTP request should not be made for invalid enum value");
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        let result = sb.eval::<Value>(r#"sdk.list_items({ status = "deleted" })"#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("expected one of"), "error was: {err}");
        assert!(err.contains("deleted"), "error was: {err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_format_validation_rejects_invalid_uuid() {
        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "testapi".to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![FunctionDef {
                name: "get_item".to_string(),
                api: "testapi".to_string(),
                tag: None,
                method: HttpMethod::Get,
                path: "/items/{id}".to_string(),
                summary: None,
                description: None,
                deprecated: false,
                parameters: vec![ParamDef {
                    name: "id".to_string(),
                    location: ParamLocation::Path,
                    param_type: ParamType::String,
                    required: true,
                    description: None,
                    default: None,
                    enum_values: None,
                    format: Some("uuid".into()),
                    frozen_value: None,
                }],
                request_body: None,
                response_schema: None,
            }],
            schemas: vec![],
            mcp_servers: vec![],
        };

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let handler = Arc::new(HttpHandler::mock(|_method, _url, _query, _body| {
            panic!("HTTP request should not be made for invalid uuid");
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        let result = sb.eval::<Value>(r#"sdk.get_item({ id = "not-a-uuid" })"#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("uuid"), "error was: {err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_frozen_param_injected() {
        let captured_query = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let captured_query_clone = Arc::clone(&captured_query);

        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "testapi".to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![FunctionDef {
                name: "list_items".to_string(),
                api: "testapi".to_string(),
                tag: None,
                method: HttpMethod::Get,
                path: "/items".to_string(),
                summary: None,
                description: None,
                deprecated: false,
                parameters: vec![
                    ParamDef {
                        name: "api_version".to_string(),
                        location: ParamLocation::Query,
                        param_type: ParamType::String,
                        required: true,
                        description: None,
                        default: None,
                        enum_values: None,
                        format: None,
                        frozen_value: Some("v2".to_string()),
                    },
                    ParamDef {
                        name: "limit".to_string(),
                        location: ParamLocation::Query,
                        param_type: ParamType::Integer,
                        required: false,
                        description: None,
                        default: None,
                        enum_values: None,
                        format: None,
                        frozen_value: None,
                    },
                ],
                request_body: None,
                response_schema: None,
            }],
            schemas: vec![],
            mcp_servers: vec![],
        };

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let handler = Arc::new(HttpHandler::mock(move |_method, _url, query, _body| {
            *captured_query_clone.lock().unwrap() = query.to_vec();
            Ok(serde_json::json!([]))
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        // Only pass limit — api_version is frozen
        sb.eval::<Value>(r"sdk.list_items({ limit = 5 })").unwrap();

        let query = captured_query.lock().unwrap().clone();
        assert!(
            query.iter().any(|(k, v)| k == "api_version" && v == "v2"),
            "Frozen param should be injected. Got: {query:?}"
        );
        assert!(
            query.iter().any(|(k, v)| k == "limit" && v == "5"),
            "Non-frozen param should come from table. Got: {query:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_all_frozen_no_body_no_args() {
        let captured_query = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let captured_query_clone = Arc::clone(&captured_query);

        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "testapi".to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![FunctionDef {
                name: "get_status".to_string(),
                api: "testapi".to_string(),
                tag: None,
                method: HttpMethod::Get,
                path: "/status".to_string(),
                summary: None,
                description: None,
                deprecated: false,
                parameters: vec![ParamDef {
                    name: "api_version".to_string(),
                    location: ParamLocation::Query,
                    param_type: ParamType::String,
                    required: true,
                    description: None,
                    default: None,
                    enum_values: None,
                    format: None,
                    frozen_value: Some("v2".to_string()),
                }],
                request_body: None,
                response_schema: None,
            }],
            schemas: vec![],
            mcp_servers: vec![],
        };

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let handler = Arc::new(HttpHandler::mock(move |_method, _url, query, _body| {
            *captured_query_clone.lock().unwrap() = query.to_vec();
            Ok(serde_json::json!({"status": "ok"}))
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        // No args at all
        sb.eval::<Value>("sdk.get_status()").unwrap();

        let query = captured_query.lock().unwrap().clone();
        assert!(
            query.iter().any(|(k, v)| k == "api_version" && v == "v2"),
            "Frozen param should still be injected. Got: {query:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_all_frozen_with_body_as_sole_arg() {
        let captured_body = Arc::new(Mutex::new(None::<serde_json::Value>));
        let captured_body_clone = Arc::clone(&captured_body);
        let captured_query = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let captured_query_clone = Arc::clone(&captured_query);

        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "testapi".to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![FunctionDef {
                name: "create_thing".to_string(),
                api: "testapi".to_string(),
                tag: None,
                method: HttpMethod::Post,
                path: "/things".to_string(),
                summary: None,
                description: None,
                deprecated: false,
                parameters: vec![ParamDef {
                    name: "api_version".to_string(),
                    location: ParamLocation::Query,
                    param_type: ParamType::String,
                    required: true,
                    description: None,
                    default: None,
                    enum_values: None,
                    format: None,
                    frozen_value: Some("v2".to_string()),
                }],
                request_body: Some(RequestBodyDef {
                    content_type: "application/json".to_string(),
                    schema: "NewThing".to_string(),
                    required: true,
                    description: None,
                }),
                response_schema: None,
            }],
            schemas: vec![],
            mcp_servers: vec![],
        };

        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let handler = Arc::new(HttpHandler::mock(move |_method, _url, query, body| {
            *captured_query_clone.lock().unwrap() = query.to_vec();
            *captured_body_clone.lock().unwrap() = body.cloned();
            Ok(serde_json::json!({"id": "1"}))
        }));
        let creds = Arc::new(AuthCredentialsMap::new());
        let counter = Arc::new(AtomicUsize::new(0));

        register_functions(&sb, &manifest, handler, creds, counter, None).unwrap();

        // Body is the sole arg (no params table since all frozen)
        sb.eval::<Value>(r#"sdk.create_thing({ name = "Widget" })"#)
            .unwrap();

        let query = captured_query.lock().unwrap().clone();
        assert!(query.iter().any(|(k, v)| k == "api_version" && v == "v2"));

        let body = captured_body.lock().unwrap().clone();
        assert!(body.is_some());
        assert_eq!(body.unwrap()["name"], "Widget");
    }

    // --- MCP tool registration tests ---

    fn mcp_manifest() -> Manifest {
        Manifest {
            apis: vec![],
            functions: vec![],
            schemas: vec![],
            mcp_servers: vec![McpServerEntry {
                name: "filesystem".to_string(),
                description: Some("File system server".to_string()),
                tools: vec![
                    McpToolDef {
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
                    },
                    McpToolDef {
                        name: "list_dir".to_string(),
                        server: "filesystem".to_string(),
                        description: Some("List a directory".to_string()),
                        params: vec![McpParamDef {
                            name: "path".to_string(),
                            luau_type: "string".to_string(),
                            required: true,
                            description: Some("Directory path".to_string()),
                            ..Default::default()
                        }],
                        schemas: vec![],
                        output_schemas: vec![],
                    },
                ],
            }],
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_register_mcp_tools_creates_sdk_tables() {
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = mcp_manifest();
        let client = Arc::new(McpClientManager::empty());
        let counter = Arc::new(AtomicUsize::new(0));

        register_mcp_tools(&sb, &manifest, client, counter, None).unwrap();

        // Verify sdk.filesystem exists and is a table
        let is_table: bool = sb
            .eval(r#"return type(sdk.filesystem) == "table""#)
            .unwrap();
        assert!(is_table, "sdk.filesystem should be a table");

        // Verify sdk.filesystem.read_file exists and is a function
        let is_fn: bool = sb
            .eval(r#"return type(sdk.filesystem.read_file) == "function""#)
            .unwrap();
        assert!(is_fn, "sdk.filesystem.read_file should be a function");

        // Verify sdk.filesystem.list_dir exists and is a function
        let is_fn: bool = sb
            .eval(r#"return type(sdk.filesystem.list_dir) == "function""#)
            .unwrap();
        assert!(is_fn, "sdk.filesystem.list_dir should be a function");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_mcp_tool_call_with_no_server_errors() {
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = mcp_manifest();
        let client = Arc::new(McpClientManager::empty());
        let counter = Arc::new(AtomicUsize::new(0));

        register_mcp_tools(&sb, &manifest, client, counter, None).unwrap();

        // Calling the tool should fail since McpClientManager::empty() has no servers
        let result =
            sb.eval::<Value>(r#"return sdk.filesystem.read_file({ path = "/tmp/test.txt" })"#);
        assert!(result.is_err(), "call should fail with empty client");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("filesystem"),
            "error should mention server name: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_mcp_tool_rejects_non_table_argument() {
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = mcp_manifest();
        let client = Arc::new(McpClientManager::empty());
        let counter = Arc::new(AtomicUsize::new(0));

        register_mcp_tools(&sb, &manifest, client, counter, None).unwrap();

        let result = sb.eval::<Value>(r#"return sdk.filesystem.read_file("bad")"#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("expected table"),
            "error should mention expected table: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_mcp_tool_api_call_limit() {
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = mcp_manifest();
        let client = Arc::new(McpClientManager::empty());
        let counter = Arc::new(AtomicUsize::new(5));

        // Set max to 5, counter already at 5
        register_mcp_tools(&sb, &manifest, client, counter, Some(5)).unwrap();

        let result =
            sb.eval::<Value>(r#"return sdk.filesystem.read_file({ path = "/tmp/test.txt" })"#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("API call limit exceeded"),
            "error should mention limit: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_mcp_tool_nil_argument_allowed() {
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let manifest = mcp_manifest();
        let client = Arc::new(McpClientManager::empty());
        let counter = Arc::new(AtomicUsize::new(0));

        register_mcp_tools(&sb, &manifest, client, counter, None).unwrap();

        // nil argument should be allowed (arguments=None), but the call will fail
        // because the empty manager has no servers — that's fine, it should fail
        // at the network level, not at argument validation.
        let result = sb.eval::<Value>("return sdk.filesystem.read_file()");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // Should NOT say "expected table" — should fail at the call_tool level
        assert!(
            !err.contains("expected table"),
            "nil arg should be accepted, not rejected as wrong type: {err}"
        );
    }

    #[test]
    fn test_convert_call_tool_result_single_json_text() {
        let lua = mlua::Lua::new();
        let result = rmcp::model::CallToolResult {
            content: vec![rmcp::model::Content::text(
                r#"{"name":"alice","age":30}"#.to_string(),
            )],
            structured_content: None,
            is_error: None,
            meta: None,
        };

        // JSON text should be returned as a string — no auto-parsing.
        // The script author uses json.decode() if needed.
        let value = convert_call_tool_result(&lua, &result).unwrap();
        match value {
            Value::String(s) => {
                assert_eq!(s.to_string_lossy(), r#"{"name":"alice","age":30}"#);
            }
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn test_convert_call_tool_result_single_plain_text() {
        let lua = mlua::Lua::new();
        let result = rmcp::model::CallToolResult {
            content: vec![rmcp::model::Content::text("hello world".to_string())],
            structured_content: None,
            is_error: None,
            meta: None,
        };

        let value = convert_call_tool_result(&lua, &result).unwrap();
        match value {
            Value::String(s) => assert_eq!(s.to_string_lossy(), "hello world"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn test_convert_call_tool_result_empty() {
        let lua = mlua::Lua::new();
        let result = rmcp::model::CallToolResult {
            content: vec![],
            structured_content: None,
            is_error: None,
            meta: None,
        };

        let value = convert_call_tool_result(&lua, &result).unwrap();
        assert!(matches!(value, Value::Nil));
    }

    #[test]
    fn test_convert_call_tool_result_multiple_texts() {
        let lua = mlua::Lua::new();
        let result = rmcp::model::CallToolResult {
            content: vec![
                rmcp::model::Content::text("line1".to_string()),
                rmcp::model::Content::text("line2".to_string()),
            ],
            structured_content: None,
            is_error: None,
            meta: None,
        };

        let value = convert_call_tool_result(&lua, &result).unwrap();
        match value {
            Value::Table(t) => {
                let v1: String = t.get(1).unwrap();
                let v2: String = t.get(2).unwrap();
                assert_eq!(v1, "line1");
                assert_eq!(v2, "line2");
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn test_convert_call_tool_result_structured_content() {
        let lua = mlua::Lua::new();
        let structured = serde_json::json!({"key": "value", "count": 42});
        let result = rmcp::model::CallToolResult {
            content: vec![rmcp::model::Content::text("ignored".to_string())],
            structured_content: Some(structured),
            is_error: None,
            meta: None,
        };

        let value = convert_call_tool_result(&lua, &result).unwrap();
        match value {
            Value::Table(t) => {
                let key: String = t.get("key").unwrap();
                assert_eq!(key, "value");
                let count: i32 = t.get("count").unwrap();
                assert_eq!(count, 42);
            }
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn test_convert_call_tool_result_is_error() {
        let result = rmcp::model::CallToolResult {
            content: vec![rmcp::model::Content::text("tool not found".to_string())],
            is_error: Some(true),
            structured_content: None,
            meta: None,
        };
        let lua = mlua::Lua::new();
        let err = convert_call_tool_result(&lua, &result);
        assert!(err.is_err(), "is_error=true should produce a Lua error");
        let err_msg = format!("{}", err.unwrap_err());
        assert!(
            err_msg.contains("MCP tool error"),
            "Error should mention MCP tool error. Got: {err_msg}"
        );
    }
}
