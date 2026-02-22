use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mlua::{LuaSerdeExt, MultiValue, Value};

use crate::codegen::manifest::{Manifest, ParamLocation};
use crate::runtime::http::{AuthCredentials, AuthCredentialsMap, HttpHandler};
use crate::runtime::sandbox::Sandbox;

/// Register all manifest functions into the sandbox's `sdk` table.
///
/// Each `FunctionDef` becomes a Lua function under `sdk.<name>` that:
/// 1. Validates required parameters
/// 2. Builds the URL with path param substitution
/// 3. Collects query parameters
/// 4. Serializes request body
/// 5. Makes the HTTP call
/// 6. Returns the response as a Lua table
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
            if let Some(max) = max_calls {
                if current_count >= max {
                    return Err(mlua::Error::external(anyhow::anyhow!(
                        "API call limit exceeded (max {} calls)",
                        max
                    )));
                }
            }

            // Extract arguments by position matching parameter order
            let arg_values: Vec<Value> = args.into_iter().collect();

            // Check if last arg is a table (request body)
            let has_body_param = func_def.request_body.is_some();

            // Build path with param substitution and collect query params
            let mut url = base_url.clone();
            let mut path = func_def.path.clone();
            let mut query_params: Vec<(String, String)> = Vec::new();

            // Total expected positional params (not counting body)
            let param_count = func_def.parameters.len();

            for (i, param) in func_def.parameters.iter().enumerate() {
                let value = if i < arg_values.len() {
                    arg_values[i].clone()
                } else {
                    Value::Nil
                };

                // Validate required params
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

                let str_value = lua_value_to_string(&value);

                match param.location {
                    ParamLocation::Path => {
                        path = path.replace(&format!("{{{}}}", param.name), &str_value);
                    }
                    ParamLocation::Query => {
                        query_params.push((param.name.clone(), str_value));
                    }
                    ParamLocation::Header => {
                        // Headers are handled at HTTP level; for now skip
                    }
                }
            }

            url.push_str(&path);

            // Extract request body if present
            let body: Option<serde_json::Value> = if has_body_param {
                let body_idx = param_count;
                if body_idx < arg_values.len() {
                    let body_val = arg_values[body_idx].clone();
                    if !matches!(body_val, Value::Nil) {
                        let json_body: serde_json::Value =
                            lua.from_value(body_val).map_err(|e| {
                                mlua::Error::external(anyhow::anyhow!(
                                    "failed to serialize request body: {}",
                                    e
                                ))
                            })?;
                        Some(json_body)
                    } else {
                        None
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
                    body.as_ref(),
                ))
            })
            .map_err(mlua::Error::external)?;

            // Convert JSON response to Lua value
            let lua_value = lua.to_value(&response).map_err(|e| {
                mlua::Error::external(anyhow::anyhow!("failed to convert response to Lua: {}", e))
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
        Value::Number(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Nil => String::new(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
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
                        },
                        ParamDef {
                            name: "limit".to_string(),
                            location: ParamLocation::Query,
                            param_type: ParamType::Integer,
                            required: false,
                            description: None,
                            default: None,
                            enum_values: None,
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
            local pet = sdk.get_pet("123")
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

        sb.eval::<Value>(r#"sdk.get_pet("456")"#).unwrap();

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

        sb.eval::<Value>(r#"sdk.list_pets("available", 10)"#)
            .unwrap();

        let query = captured_query.lock().unwrap().clone();
        assert_eq!(query.len(), 2);
        assert_eq!(query[0], ("status".to_string(), "available".to_string()));
        assert_eq!(query[1], ("limit".to_string(), "10".to_string()));
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
            "error was: {}",
            err
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
}
