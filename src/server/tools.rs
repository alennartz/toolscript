use std::borrow::Cow;
use std::sync::Arc;

use futures::FutureExt;
use rmcp::handler::server::router::tool::ToolRoute;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{CallToolResult, Content, Tool};
use serde::Deserialize;

use super::CodeMcpServer;

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
pub fn list_apis_impl(server: &CodeMcpServer) -> String {
    let apis: Vec<serde_json::Value> = server
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
    serde_json::to_string_pretty(&apis).unwrap_or_else(|_| "[]".to_string())
}

/// Implementation for `list_functions`: returns JSON array of function summaries.
pub fn list_functions_impl(server: &CodeMcpServer, api: Option<&str>, tag: Option<&str>) -> String {
    let funcs: Vec<serde_json::Value> = server
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
    serde_json::to_string_pretty(&funcs).unwrap_or_else(|_| "[]".to_string())
}

/// Implementation for `get_function_docs`: returns the full `LuaLS` annotation.
pub fn get_function_docs_impl(server: &CodeMcpServer, name: &str) -> Result<String, String> {
    server
        .annotation_cache
        .get(name)
        .cloned()
        .ok_or_else(|| format!("Function '{name}' not found"))
}

/// Implementation for `search_docs`: case-insensitive search across all documentation.
pub fn search_docs_impl(server: &CodeMcpServer, query: &str) -> String {
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

    serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string())
}

/// Implementation for `get_schema`: returns the full `LuaLS` annotation for a schema.
pub fn get_schema_impl(server: &CodeMcpServer, name: &str) -> Result<String, String> {
    server
        .schema_cache
        .get(name)
        .cloned()
        .ok_or_else(|| format!("Schema '{name}' not found"))
}

// ---- Tool route builders (wired into MCP) ----

fn make_tool(name: &str, description: &str, schema: serde_json::Value) -> Tool {
    Tool::new(
        Cow::Owned(name.to_string()),
        Cow::Owned(description.to_string()),
        rmcp::model::object(schema),
    )
}

pub fn list_apis_tool() -> ToolRoute<CodeMcpServer> {
    ToolRoute::new_dyn(
        make_tool(
            "list_apis",
            "List all loaded APIs with names, descriptions, base URLs, and endpoint counts",
            serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        ),
        |context: ToolCallContext<'_, CodeMcpServer>| {
            let result = list_apis_impl(context.service);
            std::future::ready(Ok(CallToolResult::success(vec![Content::text(result)]))).boxed()
        },
    )
}

pub fn list_functions_tool() -> ToolRoute<CodeMcpServer> {
    ToolRoute::new_dyn(
        make_tool(
            "list_functions",
            "List available SDK functions, optionally filtered by API or tag",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "api": { "type": "string", "description": "Filter by API name" },
                    "tag": { "type": "string", "description": "Filter by tag" },
                },
            }),
        ),
        |mut context: ToolCallContext<'_, CodeMcpServer>| {
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

pub fn get_function_docs_tool() -> ToolRoute<CodeMcpServer> {
    ToolRoute::new_dyn(
        make_tool(
            "get_function_docs",
            "Get the full LuaLS annotation documentation for a specific function",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Function name" },
                },
                "required": ["name"],
            }),
        ),
        |mut context: ToolCallContext<'_, CodeMcpServer>| {
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

pub fn search_docs_tool() -> ToolRoute<CodeMcpServer> {
    ToolRoute::new_dyn(
        make_tool(
            "search_docs",
            "Search across all SDK documentation (function names, summaries, parameters, schemas)",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query" },
                },
                "required": ["query"],
            }),
        ),
        |mut context: ToolCallContext<'_, CodeMcpServer>| {
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

pub fn get_schema_tool() -> ToolRoute<CodeMcpServer> {
    ToolRoute::new_dyn(
        make_tool(
            "get_schema",
            "Get the full LuaLS annotation documentation for a schema (class/type)",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Schema name" },
                },
                "required": ["name"],
            }),
        ),
        |mut context: ToolCallContext<'_, CodeMcpServer>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: Result<NameParam, _> =
                serde_json::from_value(serde_json::Value::Object(args));
            let result = match params {
                Ok(p) => match get_schema_impl(context.service, &p.name) {
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

pub fn execute_script_tool() -> ToolRoute<CodeMcpServer> {
    ToolRoute::new_dyn(
        execute_script_tool_def(),
        |mut context: ToolCallContext<'_, CodeMcpServer>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: Result<ExecuteScriptParams, _> =
                serde_json::from_value(serde_json::Value::Object(args));
            execute_script_async(params, context.service).boxed()
        },
    )
}

fn execute_script_tool_def() -> Tool {
    make_tool(
        "execute_script",
        "Execute a Lua script against the SDK. Auth comes from server-side configuration.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "script": { "type": "string", "description": "Lua script to execute" },
                "timeout_ms": { "type": "integer", "description": "Execution timeout in milliseconds (optional)" },
            },
            "required": ["script"],
        }),
    )
}

async fn execute_script_async(
    params: Result<ExecuteScriptParams, serde_json::Error>,
    server: &CodeMcpServer,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let params = match params {
        Ok(p) => p,
        Err(e) => {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Invalid params: {e}"
            ))]));
        }
    };

    let auth = &server.auth;
    let result = server
        .executor
        .execute(&params.script, auth, params.timeout_ms)
        .await;

    match result {
        Ok(exec_result) => {
            let response = serde_json::json!({
                "result": exec_result.result,
                "logs": exec_result.logs,
                "stats": {
                    "api_calls": exec_result.stats.api_calls,
                    "duration_ms": exec_result.stats.duration_ms,
                }
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

// ---- Arc<CodeMcpServer> tool variants for HTTP transport ----
// When using StreamableHttpService, the service factory creates new Router<Arc<CodeMcpServer>>
// instances. These tool routes work with Arc<CodeMcpServer> instead of CodeMcpServer.

pub fn list_apis_tool_arc() -> ToolRoute<Arc<CodeMcpServer>> {
    ToolRoute::new_dyn(
        make_tool(
            "list_apis",
            "List all loaded APIs with names, descriptions, base URLs, and endpoint counts",
            serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        ),
        |context: ToolCallContext<'_, Arc<CodeMcpServer>>| {
            let result = list_apis_impl(context.service);
            std::future::ready(Ok(CallToolResult::success(vec![Content::text(result)]))).boxed()
        },
    )
}

pub fn list_functions_tool_arc() -> ToolRoute<Arc<CodeMcpServer>> {
    ToolRoute::new_dyn(
        make_tool(
            "list_functions",
            "List available SDK functions, optionally filtered by API or tag",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "api": { "type": "string", "description": "Filter by API name" },
                    "tag": { "type": "string", "description": "Filter by tag" },
                },
            }),
        ),
        |mut context: ToolCallContext<'_, Arc<CodeMcpServer>>| {
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

pub fn get_function_docs_tool_arc() -> ToolRoute<Arc<CodeMcpServer>> {
    ToolRoute::new_dyn(
        make_tool(
            "get_function_docs",
            "Get the full LuaLS annotation documentation for a specific function",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Function name" },
                },
                "required": ["name"],
            }),
        ),
        |mut context: ToolCallContext<'_, Arc<CodeMcpServer>>| {
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

pub fn search_docs_tool_arc() -> ToolRoute<Arc<CodeMcpServer>> {
    ToolRoute::new_dyn(
        make_tool(
            "search_docs",
            "Search across all SDK documentation (function names, summaries, parameters, schemas)",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query" },
                },
                "required": ["query"],
            }),
        ),
        |mut context: ToolCallContext<'_, Arc<CodeMcpServer>>| {
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

pub fn get_schema_tool_arc() -> ToolRoute<Arc<CodeMcpServer>> {
    ToolRoute::new_dyn(
        make_tool(
            "get_schema",
            "Get the full LuaLS annotation documentation for a schema (class/type)",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Schema name" },
                },
                "required": ["name"],
            }),
        ),
        |mut context: ToolCallContext<'_, Arc<CodeMcpServer>>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: Result<NameParam, _> =
                serde_json::from_value(serde_json::Value::Object(args));
            let result = match params {
                Ok(p) => match get_schema_impl(context.service, &p.name) {
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

pub fn execute_script_tool_arc() -> ToolRoute<Arc<CodeMcpServer>> {
    ToolRoute::new_dyn(
        execute_script_tool_def(),
        |mut context: ToolCallContext<'_, Arc<CodeMcpServer>>| {
            let args = context.arguments.take().unwrap_or_default();
            let params: Result<ExecuteScriptParams, _> =
                serde_json::from_value(serde_json::Value::Object(args));
            execute_script_async(params, context.service).boxed()
        },
    )
}
