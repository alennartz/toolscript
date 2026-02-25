use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use mlua::{LuaSerdeExt, Value, VmState};

use crate::codegen::manifest::Manifest;
use crate::runtime::http::{AuthCredentialsMap, HttpHandler};
use crate::runtime::registry;
use crate::runtime::sandbox::{Sandbox, SandboxConfig};

/// Configuration for the script executor.
pub struct ExecutorConfig {
    /// Execution timeout in milliseconds. Default: 30000 (30s).
    pub timeout_ms: u64,
    /// Maximum memory the Lua VM may allocate (in bytes). Default: 64 MB.
    pub memory_limit: Option<usize>,
    /// Maximum number of API calls per script execution. Default: 100.
    pub max_api_calls: Option<usize>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30000,
            memory_limit: Some(64 * 1024 * 1024),
            max_api_calls: Some(100),
        }
    }
}

/// Result of executing a Lua script.
#[derive(Debug)]
pub struct ExecutionResult {
    /// The return value of the script, serialized as JSON.
    pub result: serde_json::Value,
    /// Captured log output from `print()` calls.
    pub logs: Vec<String>,
    /// Execution statistics.
    pub stats: ExecutionStats,
}

/// Statistics about a script execution.
#[derive(Debug)]
pub struct ExecutionStats {
    /// Number of API calls made during execution.
    pub api_calls: usize,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

/// Orchestrates script execution: creates sandbox, registers SDK, runs script.
pub struct ScriptExecutor {
    manifest: Manifest,
    handler: Arc<HttpHandler>,
    config: ExecutorConfig,
}

impl ScriptExecutor {
    /// Create a new executor.
    pub const fn new(
        manifest: Manifest,
        handler: Arc<HttpHandler>,
        config: ExecutorConfig,
    ) -> Self {
        Self {
            manifest,
            handler,
            config,
        }
    }

    /// Execute a Lua script against the SDK.
    ///
    /// Creates a fresh sandbox per execution for isolation, registers SDK
    /// functions, and executes the script with timeout and API call limits.
    ///
    /// If `timeout_ms` is provided, it overrides the default timeout from the
    /// executor configuration for this single execution.
    #[allow(clippy::unused_async)] // async is part of the public API contract
    pub async fn execute(
        &self,
        script: &str,
        auth: &AuthCredentialsMap,
        timeout_ms: Option<u64>,
    ) -> anyhow::Result<ExecutionResult> {
        let start = Instant::now();

        // 1. Create fresh sandbox
        let sandbox = Sandbox::new(SandboxConfig {
            memory_limit: self.config.memory_limit,
        })?;

        // 2. Set up API call counter
        let api_call_counter = Arc::new(AtomicUsize::new(0));

        // 3. Register SDK functions
        registry::register_functions(
            &sandbox,
            &self.manifest,
            Arc::clone(&self.handler),
            Arc::new(auth.clone()),
            Arc::clone(&api_call_counter),
            self.config.max_api_calls,
        )?;

        // 3b. Enable Luau sandbox mode now that all globals are set up
        sandbox.enable_sandbox()?;

        // 4. Set up timeout via Luau interrupt
        let effective_timeout = timeout_ms.unwrap_or(self.config.timeout_ms);
        let deadline = Instant::now() + std::time::Duration::from_millis(effective_timeout);
        sandbox.lua().set_interrupt(move |_lua| {
            if Instant::now() >= deadline {
                Err(mlua::Error::external(anyhow::anyhow!(
                    "script execution timed out"
                )))
            } else {
                Ok(VmState::Continue)
            }
        });

        // 5. Execute the script
        let script_owned = script.to_string();
        let lua_result =
            tokio::task::block_in_place(|| sandbox.lua().load(&script_owned).eval::<Value>());

        // 6. Collect logs
        let logs = sandbox.take_logs();

        // 7. Convert result to JSON
        let result_json = match lua_result {
            Ok(value) => lua_value_to_json(sandbox.lua(), value)?,
            Err(e) => {
                return Err(anyhow::anyhow!("{e}"));
            }
        };

        #[allow(clippy::cast_possible_truncation)] // duration will not exceed u64::MAX ms
        let duration_ms = start.elapsed().as_millis() as u64;
        let api_calls = api_call_counter.load(Ordering::SeqCst);

        Ok(ExecutionResult {
            result: result_json,
            logs,
            stats: ExecutionStats {
                api_calls,
                duration_ms,
            },
        })
    }
}

/// Convert a Lua `Value` to `serde_json::Value`.
fn lua_value_to_json(lua: &mlua::Lua, value: Value) -> anyhow::Result<serde_json::Value> {
    match value {
        Value::Boolean(b) => Ok(serde_json::Value::Bool(b)),
        Value::Integer(n) => Ok(serde_json::json!(n)),
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        Value::Number(n) => {
            if n.fract() == 0.0 && (i64::MIN as f64..=i64::MAX as f64).contains(&n) {
                Ok(serde_json::json!(n as i64))
            } else {
                Ok(serde_json::json!(n))
            }
        }
        Value::String(s) => Ok(serde_json::Value::String(s.to_string_lossy())),
        Value::Table(_) => {
            let json: serde_json::Value = lua.from_value(value)?;
            Ok(json)
        }
        _ => Ok(serde_json::Value::Null),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::codegen::manifest::*;
    use crate::runtime::http::HttpHandler;

    fn test_manifest() -> Manifest {
        Manifest {
            apis: vec![ApiConfig {
                name: "petstore".to_string(),
                base_url: "https://petstore.example.com/v1".to_string(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![FunctionDef {
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
            }],
            schemas: vec![],
        }
    }

    fn empty_manifest() -> Manifest {
        Manifest {
            apis: vec![],
            functions: vec![],
            schemas: vec![],
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_returns_result() {
        let executor = ScriptExecutor::new(
            empty_manifest(),
            Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
            ExecutorConfig::default(),
        );
        let auth = AuthCredentialsMap::new();

        let result = executor.execute("return 42", &auth, None).await.unwrap();
        assert_eq!(result.result, serde_json::json!(42));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_captures_logs() {
        let executor = ScriptExecutor::new(
            empty_manifest(),
            Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
            ExecutorConfig::default(),
        );
        let auth = AuthCredentialsMap::new();

        let result = executor
            .execute(
                r#"
                print("hello")
                print("world")
                return true
            "#,
                &auth,
                None,
            )
            .await
            .unwrap();

        assert_eq!(result.logs, vec!["hello", "world"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_with_sdk_calls() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);

        let executor = ScriptExecutor::new(
            test_manifest(),
            Arc::new(HttpHandler::mock(move |_, _, _, _| {
                call_count_clone.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({"id": "123", "name": "Fido"}))
            })),
            ExecutorConfig::default(),
        );
        let auth = AuthCredentialsMap::new();

        let result = executor
            .execute(
                r#"
                local pet = sdk.get_pet({ pet_id = "123" })
                return pet.name
            "#,
                &auth,
                None,
            )
            .await
            .unwrap();

        assert_eq!(result.result, serde_json::json!("Fido"));
        assert!(result.stats.api_calls >= 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_timeout() {
        let executor = ScriptExecutor::new(
            empty_manifest(),
            Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
            ExecutorConfig {
                timeout_ms: 50, // very short timeout
                memory_limit: Some(64 * 1024 * 1024),
                max_api_calls: Some(100),
            },
        );
        let auth = AuthCredentialsMap::new();

        let result = executor.execute("while true do end", &auth, None).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timed out") || err.contains("timeout"),
            "error was: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_parse_error() {
        let executor = ScriptExecutor::new(
            empty_manifest(),
            Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
            ExecutorConfig::default(),
        );
        let auth = AuthCredentialsMap::new();

        let result = executor
            .execute("this is not valid lua @@@@", &auth, None)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_tracks_api_calls() {
        let executor = ScriptExecutor::new(
            test_manifest(),
            Arc::new(HttpHandler::mock(|_, _, _, _| {
                Ok(serde_json::json!({"id": "1"}))
            })),
            ExecutorConfig::default(),
        );
        let auth = AuthCredentialsMap::new();

        let result = executor
            .execute(
                r#"
                sdk.get_pet({ pet_id = "1" })
                sdk.get_pet({ pet_id = "2" })
                sdk.get_pet({ pet_id = "3" })
                return "done"
            "#,
                &auth,
                None,
            )
            .await
            .unwrap();

        assert_eq!(result.stats.api_calls, 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_fresh_sandbox() {
        let executor = ScriptExecutor::new(
            empty_manifest(),
            Arc::new(HttpHandler::mock(|_, _, _, _| Ok(serde_json::json!({})))),
            ExecutorConfig::default(),
        );
        let auth = AuthCredentialsMap::new();

        // First execution sets a global
        executor
            .execute("my_global = 42; return my_global", &auth, None)
            .await
            .unwrap();

        // Second execution should NOT see it
        let result = executor
            .execute("return type(my_global)", &auth, None)
            .await
            .unwrap();

        // In a fresh sandbox, my_global should be nil
        // type(nil) returns "nil" as a string
        assert_eq!(result.result, serde_json::json!("nil"));
    }
}
