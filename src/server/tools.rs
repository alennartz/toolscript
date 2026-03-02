use std::borrow::Cow;
use std::sync::Arc;

use futures::FutureExt;
use rmcp::handler::server::router::tool::ToolRoute;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{CallToolResult, Content, Tool};
use serde::Deserialize;

use super::ToolScriptServer;
use super::auth;
use super::builtins;
use crate::runtime::http::AuthCredentialsMap;

// ---- Tool parameter structs ----

#[derive(Deserialize, Default)]
struct ListFunctionsParams {
    api: Option<String>,
    tag: Option<String>,
}

#[derive(Deserialize)]
struct NameParam {
    name: String,
}

#[derive(Deserialize)]
struct QueryParam {
    query: String,
}

#[derive(Deserialize)]
struct ExecuteScriptParams {
    script: String,
    timeout_ms: Option<u64>,
}

// ---- Tool implementations (pure logic, testable without MCP protocol) ----

/// Implementation for `list_apis`: returns JSON array of API summaries.
pub fn list_apis_impl(server: &ToolScriptServer) -> String {
    let mut apis: Vec<serde_json::Value> = server
        .manifest
        .apis
        .iter()
        .map(|api| {
            let function_count = server
                .manifest
                .functions
                .iter()
                .filter(|f| f.api == api.name)
                .count();
            serde_json::json!({
                "name": api.name,
                "description": api.description,
                "version": api.version,
                "base_url": api.base_url,
                "function_count": function_count,
            })
        })
        .collect();

    // Append MCP servers
    for mcp_server in &server.manifest.mcp_servers {
        apis.push(serde_json::json!({
            "name": mcp_server.name,
            "description": mcp_server.description,
            "source": "mcp",
            "tool_count": mcp_server.tools.len(),
        }));
    }

    // Append built-in Luau globals
    let builtin_count = builtins::builtin_functions(server.io_enabled).count();
    apis.push(serde_json::json!({
        "name": "luau",
        "description": builtins::LUAU_DESCRIPTION,
        "source": "builtin",
        "function_count": builtin_count,
    }));

    serde_json::to_string_pretty(&apis).unwrap_or_else(|_| "[]".to_string())
}

/// Implementation for `list_functions`: returns JSON array of function summaries.
pub fn list_functions_impl(
    server: &ToolScriptServer,
    api: Option<&str>,
    tag: Option<&str>,
) -> String {
    let mut funcs: Vec<serde_json::Value> = server
        .manifest
        .functions
        .iter()
        .filter(|f| {
            if let Some(api_filter) = api
                && f.api != api_filter
            {
                return false;
            }
            tag.is_none_or(|tag_filter| f.tag.as_ref().is_some_and(|t| t == tag_filter))
        })
        .map(|f| {
            serde_json::json!({
                "name": f.name,
                "summary": f.summary,
                "api": f.api,
                "tag": f.tag,
                "deprecated": f.deprecated,
            })
        })
        .collect();

    // Append MCP tools (skip when filtering by tag, since MCP tools have no tags)
    if tag.is_none() {
        for mcp_server in &server.manifest.mcp_servers {
            if let Some(api_filter) = api
                && mcp_server.name != api_filter
            {
                continue;
            }
            for tool in &mcp_server.tools {
                funcs.push(serde_json::json!({
                    "name": tool.name,
                    "summary": tool.description,
                    "api": mcp_server.name,
                    "source": "mcp",
                    "deprecated": false,
                }));
            }
        }
    }

    // Append built-in Luau globals (skip when filtering by tag, since builtins have no tags)
    if tag.is_none() {
        for builtin in builtins::builtin_functions(server.io_enabled) {
            if let Some(api_filter) = api
                && api_filter != "luau"
            {
                continue;
            }
            funcs.push(serde_json::json!({
                "name": builtin.name,
                "summary": builtin.summary,
                "api": "luau",
                "source": "builtin",
                "deprecated": false,
            }));
        }
    }

    serde_json::to_string_pretty(&funcs).unwrap_or_else(|_| "[]".to_string())
}

/// Implementation for `get_function_docs`: returns the full Luau type annotation.
pub fn get_function_docs_impl(server: &ToolScriptServer, name: &str) -> Result<String, String> {
    server
        .annotation_cache
        .get(name)
        .cloned()
        .ok_or_else(|| format!("Function '{name}' not found"))
}

/// Implementation for `search_docs`: case-insensitive search across all documentation.
#[allow(clippy::too_many_lines)]
pub fn search_docs_impl(server: &ToolScriptServer, query: &str) -> String {
    let query_lower = query.to_lowercase();
    let mut results: Vec<serde_json::Value> = Vec::new();

    // Search functions
    for func in &server.manifest.functions {
        let mut matches = false;
        let mut context = Vec::new();

        if func.name.to_lowercase().contains(&query_lower) {
            matches = true;
            context.push(format!("name: {}", func.name));
        }
        if let Some(ref summary) = func.summary
            && summary.to_lowercase().contains(&query_lower)
        {
            matches = true;
            context.push(format!("summary: {summary}"));
        }
        if let Some(ref desc) = func.description
            && desc.to_lowercase().contains(&query_lower)
        {
            matches = true;
            context.push(format!("description: {desc}"));
        }
        for param in &func.parameters {
            if param.frozen_value.is_some() {
                continue; // Skip frozen params from search
            }
            if param.name.to_lowercase().contains(&query_lower) {
                matches = true;
                context.push(format!("parameter: {}", param.name));
            }
        }

        if matches {
            results.push(serde_json::json!({
                "type": "function",
                "name": func.name,
                "api": func.api,
                "context": context,
            }));
        }
    }

    // Search schemas
    for schema in &server.manifest.schemas {
        let mut matches = false;
        let mut context = Vec::new();

        if schema.name.to_lowercase().contains(&query_lower) {
            matches = true;
            context.push(format!("name: {}", schema.name));
        }
        if let Some(ref desc) = schema.description
            && desc.to_lowercase().contains(&query_lower)
        {
            matches = true;
            context.push(format!("description: {desc}"));
        }
        for field in &schema.fields {
            if field.name.to_lowercase().contains(&query_lower) {
                matches = true;
                context.push(format!("field: {}", field.name));
            }
        }

        if matches {
            results.push(serde_json::json!({
                "type": "schema",
                "name": schema.name,
                "context": context,
            }));
        }
    }

    // Search MCP tools
    for mcp_server in &server.manifest.mcp_servers {
        for tool in &mcp_server.tools {
            let mut matches = false;
            let mut context = Vec::new();

            let full_name = format!("{}.{}", mcp_server.name, tool.name);
            if full_name.to_lowercase().contains(&query_lower)
                || tool.name.to_lowercase().contains(&query_lower)
            {
                matches = true;
                context.push(format!("name: {full_name}"));
            }
            if let Some(ref desc) = tool.description
                && desc.to_lowercase().contains(&query_lower)
            {
                matches = true;
                context.push(format!("description: {desc}"));
            }
            for param in &tool.params {
                if param.name.to_lowercase().contains(&query_lower) {
                    matches = true;
                    context.push(format!("parameter: {}", param.name));
                }
            }

            if matches {
                results.push(serde_json::json!({
                    "type": "mcp_tool",
                    "name": full_name,
                    "server": mcp_server.name,
                    "context": context,
                }));
            }
        }
    }

    // Search built-in Luau globals
    for builtin in builtins::builtin_functions(server.io_enabled) {
        let mut matches = false;
        let mut context = Vec::new();

        if builtin.name.to_lowercase().contains(&query_lower) {
            matches = true;
            context.push(format!("name: {}", builtin.name));
        }
        if builtin.summary.to_lowercase().contains(&query_lower) {
            matches = true;
            context.push(format!("summary: {}", builtin.summary));
        }
        if builtin.annotation.to_lowercase().contains(&query_lower) {
            matches = true;
            context.push("annotation match".to_string());
        }

        if matches {
            results.push(serde_json::json!({
                "type": "builtin",
                "name": builtin.name,
                "api": "luau",
                "context": context,
            }));
        }
    }

    serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string())
}

// ---- Tool route builders (wired into MCP) ----

fn make_tool(name: &str, description: &str, schema: serde_json::Value) -> Tool {
    Tool::new(
        Cow::Owned(name.to_string()),
        Cow::Owned(description.to_string()),
        rmcp::model::object(schema),
    )
}

fn list_apis_tool_def() -> Tool {
    make_tool(
        "list_apis",
        "List available APIs. Returns a JSON array where each entry has: name, description, and function count. Use list_functions to see the functions within an API.",
        serde_json::json!({
            "type": "object",
            "properties": {},
        }),
    )
}

fn list_functions_tool_def() -> Tool {
    make_tool(
        "list_functions",
        "List available SDK functions. Returns a JSON array with name, summary, api, and tag for each function. Use get_function_docs to see the full Luau type signature and calling convention.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "api": { "type": "string", "description": "Filter by API name" },
                "tag": { "type": "string", "description": "Filter by tag" },
            },
        }),
    )
}

fn get_function_docs_tool_def() -> Tool {
    make_tool(
        "get_function_docs",
        "Get the Luau type annotation for a function, showing its call signature, parameter types, and referenced types. Pass the name as shown by list_functions.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Function name" },
            },
            "required": ["name"],
        }),
    )
}

fn search_docs_tool_def() -> Tool {
    make_tool(
        "search_docs",
        "Search SDK documentation by keyword. Matches against function names, summaries, parameter names, and schema fields. Returns a JSON array of matches with context showing where the query matched.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
            },
            "required": ["query"],
        }),
    )
}

pub fn list_apis_tool() -> ToolRoute<ToolScriptServer> {
    ToolRoute::new_dyn(
        list_apis_tool_def(),
        |context: ToolCallContext<'_, ToolScriptServer>| {
            let result = list_apis_impl(context.service);
            std::future::ready(Ok(CallToolResult::success(vec![Content::text(result)]))).boxed()
        },
    )
}

pub fn list_functions_tool() -> ToolRoute<ToolScriptServer> {
    ToolRoute::new_dyn(
        list_functions_tool_def(),
        |mut context: ToolCallContext<'_, ToolScriptServer>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: ListFunctionsParams =
                serde_json::from_value(serde_json::Value::Object(args)).unwrap_or_default();
            let result = list_functions_impl(
                context.service,
                params.api.as_deref(),
                params.tag.as_deref(),
            );
            std::future::ready(Ok(CallToolResult::success(vec![Content::text(result)]))).boxed()
        },
    )
}

pub fn get_function_docs_tool() -> ToolRoute<ToolScriptServer> {
    ToolRoute::new_dyn(
        get_function_docs_tool_def(),
        |mut context: ToolCallContext<'_, ToolScriptServer>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: Result<NameParam, _> =
                serde_json::from_value(serde_json::Value::Object(args));
            let result = match params {
                Ok(p) => match get_function_docs_impl(context.service, &p.name) {
                    Ok(docs) => CallToolResult::success(vec![Content::text(docs)]),
                    Err(e) => CallToolResult::error(vec![Content::text(e)]),
                },
                Err(e) => {
                    CallToolResult::error(vec![Content::text(format!("Invalid params: {e}"))])
                }
            };
            std::future::ready(Ok(result)).boxed()
        },
    )
}

pub fn search_docs_tool() -> ToolRoute<ToolScriptServer> {
    ToolRoute::new_dyn(
        search_docs_tool_def(),
        |mut context: ToolCallContext<'_, ToolScriptServer>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: Result<QueryParam, _> =
                serde_json::from_value(serde_json::Value::Object(args));
            let result = match params {
                Ok(p) => {
                    let results = search_docs_impl(context.service, &p.query);
                    CallToolResult::success(vec![Content::text(results)])
                }
                Err(e) => {
                    CallToolResult::error(vec![Content::text(format!("Invalid params: {e}"))])
                }
            };
            std::future::ready(Ok(result)).boxed()
        },
    )
}

pub fn execute_script_tool() -> ToolRoute<ToolScriptServer> {
    ToolRoute::new_dyn(
        execute_script_tool_def(),
        |mut context: ToolCallContext<'_, ToolScriptServer>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: Result<ExecuteScriptParams, _> =
                serde_json::from_value(serde_json::Value::Object(args));
            let meta_auth = context
                .request_context
                .meta
                .get("auth")
                .map_or_else(AuthCredentialsMap::new, |auth_value| {
                    auth::parse_meta_auth(auth_value)
                });
            execute_script_async(params, context.service, meta_auth).boxed()
        },
    )
}

fn execute_script_tool_def() -> Tool {
    make_tool(
        "execute_script",
        "Execute a Luau script against the SDK. Auth comes from server-side configuration.\n\n\
         Returns a JSON object with:\n\
         - result: the script's return value (any JSON type)\n\
         - logs: array of strings captured from print() calls\n\
         - files_touched: array of { name, op, bytes } for files modified via io/os\n\n\
         On error, returns a text message prefixed with \"Script execution error:\".\n\n\
         Only a subset of Lua globals are available in the sandbox. \
         Use list_functions(api: \"luau\") or browse sdk://luau/functions to see built-in functions and their signatures.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "script": { "type": "string", "description": "Luau script to execute" },
                "timeout_ms": { "type": "integer", "description": "Execution timeout in milliseconds (optional)" },
            },
            "required": ["script"],
        }),
    )
}

async fn execute_script_async(
    params: Result<ExecuteScriptParams, serde_json::Error>,
    server: &ToolScriptServer,
    meta_auth: AuthCredentialsMap,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let params = match params {
        Ok(p) => p,
        Err(e) => {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Invalid params: {e}"
            ))]));
        }
    };

    let merged_auth = auth::merge_credentials(&server.auth, &meta_auth);
    let result = server
        .executor
        .execute(&params.script, &merged_auth, params.timeout_ms)
        .await;

    match result {
        Ok(exec_result) => {
            let response = serde_json::json!({
                "result": exec_result.result,
                "logs": exec_result.logs,
                "files_touched": exec_result.files_touched.iter().map(|f| {
                    serde_json::json!({
                        "name": f.name,
                        "op": f.op,
                        "bytes": f.bytes,
                    })
                }).collect::<Vec<_>>(),
            });
            Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&response).unwrap_or_default(),
            )]))
        }
        Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
            "Script execution error: {e}"
        ))])),
    }
}

// ---- Arc<ToolScriptServer> tool variants for HTTP transport ----
// When using StreamableHttpService, the service factory creates new Router<Arc<ToolScriptServer>>
// instances. These tool routes work with Arc<ToolScriptServer> instead of ToolScriptServer.

pub fn list_apis_tool_arc() -> ToolRoute<Arc<ToolScriptServer>> {
    ToolRoute::new_dyn(
        list_apis_tool_def(),
        |context: ToolCallContext<'_, Arc<ToolScriptServer>>| {
            let result = list_apis_impl(context.service);
            std::future::ready(Ok(CallToolResult::success(vec![Content::text(result)]))).boxed()
        },
    )
}

pub fn list_functions_tool_arc() -> ToolRoute<Arc<ToolScriptServer>> {
    ToolRoute::new_dyn(
        list_functions_tool_def(),
        |mut context: ToolCallContext<'_, Arc<ToolScriptServer>>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: ListFunctionsParams =
                serde_json::from_value(serde_json::Value::Object(args)).unwrap_or_default();
            let result = list_functions_impl(
                context.service,
                params.api.as_deref(),
                params.tag.as_deref(),
            );
            std::future::ready(Ok(CallToolResult::success(vec![Content::text(result)]))).boxed()
        },
    )
}

pub fn get_function_docs_tool_arc() -> ToolRoute<Arc<ToolScriptServer>> {
    ToolRoute::new_dyn(
        get_function_docs_tool_def(),
        |mut context: ToolCallContext<'_, Arc<ToolScriptServer>>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: Result<NameParam, _> =
                serde_json::from_value(serde_json::Value::Object(args));
            let result = match params {
                Ok(p) => match get_function_docs_impl(context.service, &p.name) {
                    Ok(docs) => CallToolResult::success(vec![Content::text(docs)]),
                    Err(e) => CallToolResult::error(vec![Content::text(e)]),
                },
                Err(e) => {
                    CallToolResult::error(vec![Content::text(format!("Invalid params: {e}"))])
                }
            };
            std::future::ready(Ok(result)).boxed()
        },
    )
}

pub fn search_docs_tool_arc() -> ToolRoute<Arc<ToolScriptServer>> {
    ToolRoute::new_dyn(
        search_docs_tool_def(),
        |mut context: ToolCallContext<'_, Arc<ToolScriptServer>>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: Result<QueryParam, _> =
                serde_json::from_value(serde_json::Value::Object(args));
            let result = match params {
                Ok(p) => {
                    let results = search_docs_impl(context.service, &p.query);
                    CallToolResult::success(vec![Content::text(results)])
                }
                Err(e) => {
                    CallToolResult::error(vec![Content::text(format!("Invalid params: {e}"))])
                }
            };
            std::future::ready(Ok(result)).boxed()
        },
    )
}

pub fn execute_script_tool_arc() -> ToolRoute<Arc<ToolScriptServer>> {
    ToolRoute::new_dyn(
        execute_script_tool_def(),
        |mut context: ToolCallContext<'_, Arc<ToolScriptServer>>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: Result<ExecuteScriptParams, _> =
                serde_json::from_value(serde_json::Value::Object(args));
            let meta_auth = context
                .request_context
                .meta
                .get("auth")
                .map_or_else(AuthCredentialsMap::new, |auth_value| {
                    auth::parse_meta_auth(auth_value)
                });
            execute_script_async(params, context.service, meta_auth).boxed()
        },
    )
}
