use std::collections::HashMap;
use std::sync::Arc;

use crate::codegen::manifest::{AuthConfig, Manifest};

/// Authentication credentials for a single API.
#[derive(Clone, Debug)]
pub enum AuthCredentials {
    BearerToken(String),
    ApiKey(String),
    Basic { username: String, password: String },
    None,
}

/// Map from API name to its credentials.
pub type AuthCredentialsMap = HashMap<String, AuthCredentials>;

/// Mock function signature: (method, url, query_params, body) -> Result<serde_json::Value>
type MockFn = Arc<
    dyn Fn(&str, &str, &[(String, String)], Option<&serde_json::Value>) -> anyhow::Result<serde_json::Value>
        + Send
        + Sync,
>;

/// HTTP handler that makes real requests or uses a mock for testing.
#[derive(Clone)]
pub struct HttpHandler {
    inner: HttpHandlerInner,
}

#[derive(Clone)]
enum HttpHandlerInner {
    Real(reqwest::Client),
    Mock(MockFn),
}

impl HttpHandler {
    /// Create a real HTTP handler.
    pub fn new() -> Self {
        Self {
            inner: HttpHandlerInner::Real(reqwest::Client::new()),
        }
    }

    /// Create a mock HTTP handler for testing.
    pub fn mock<F>(f: F) -> Self
    where
        F: Fn(&str, &str, &[(String, String)], Option<&serde_json::Value>) -> anyhow::Result<serde_json::Value>
            + Send
            + Sync
            + 'static,
    {
        Self {
            inner: HttpHandlerInner::Mock(Arc::new(f)),
        }
    }

    /// Make an HTTP request with auth injection.
    pub async fn request(
        &self,
        method: &str,
        url: &str,
        auth_config: Option<&AuthConfig>,
        credentials: &AuthCredentials,
        query_params: &[(String, String)],
        body: Option<&serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        match &self.inner {
            HttpHandlerInner::Mock(f) => f(method, url, query_params, body),
            HttpHandlerInner::Real(client) => {
                let req_method = method.parse::<reqwest::Method>().map_err(|e| {
                    anyhow::anyhow!("invalid HTTP method '{}': {}", method, e)
                })?;

                let mut builder = client.request(req_method, url);

                // Add query parameters
                if !query_params.is_empty() {
                    builder = builder.query(query_params);
                }

                // Inject authentication
                builder = inject_auth(builder, auth_config, credentials);

                // Add request body
                if let Some(body) = body {
                    builder = builder
                        .header("Content-Type", "application/json")
                        .json(body);
                }

                let response = builder.send().await?;
                let status = response.status();

                if !status.is_success() {
                    let body_text = response.text().await.unwrap_or_default();
                    return Err(anyhow::anyhow!(
                        "HTTP {} {}: {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or(""),
                        body_text
                    ));
                }

                let json: serde_json::Value = response.json().await?;
                Ok(json)
            }
        }
    }
}

/// Load authentication credentials from environment variables for each API in the manifest.
///
/// For each API, checks environment variables in order of precedence:
/// 1. `{API_NAME}_BEARER_TOKEN` -> BearerToken
/// 2. `{API_NAME}_API_KEY` -> ApiKey
/// 3. `{API_NAME}_BASIC_USER` + `{API_NAME}_BASIC_PASS` -> Basic
///
/// The API name is converted to UPPERCASE for the env var prefix.
pub fn load_auth_from_env(manifest: &Manifest) -> AuthCredentialsMap {
    let mut map = HashMap::new();
    for api in &manifest.apis {
        let prefix = api.name.to_uppercase();
        if let Ok(token) = std::env::var(format!("{prefix}_BEARER_TOKEN")) {
            map.insert(api.name.clone(), AuthCredentials::BearerToken(token));
        } else if let Ok(key) = std::env::var(format!("{prefix}_API_KEY")) {
            map.insert(api.name.clone(), AuthCredentials::ApiKey(key));
        } else if let (Ok(user), Ok(pass)) = (
            std::env::var(format!("{prefix}_BASIC_USER")),
            std::env::var(format!("{prefix}_BASIC_PASS")),
        ) {
            map.insert(
                api.name.clone(),
                AuthCredentials::Basic {
                    username: user,
                    password: pass,
                },
            );
        }
    }
    map
}

/// Inject authentication into the request builder based on config + credentials.
fn inject_auth(
    mut builder: reqwest::RequestBuilder,
    auth_config: Option<&AuthConfig>,
    credentials: &AuthCredentials,
) -> reqwest::RequestBuilder {
    match (auth_config, credentials) {
        (Some(AuthConfig::Bearer { header, prefix }), AuthCredentials::BearerToken(token)) => {
            let value = format!("{}{}", prefix, token);
            builder = builder.header(header.as_str(), value);
        }
        (Some(AuthConfig::ApiKey { header }), AuthCredentials::ApiKey(key)) => {
            builder = builder.header(header.as_str(), key.as_str());
        }
        (Some(AuthConfig::Basic), AuthCredentials::Basic { username, password }) => {
            builder = builder.basic_auth(username, Some(password));
        }
        _ => {}
    }
    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::manifest::ApiConfig;

    #[tokio::test]
    async fn test_mock_handler_returns_response() {
        let handler = HttpHandler::mock(|_method, _url, _query, _body| {
            Ok(serde_json::json!({"id": "123", "name": "Fido"}))
        });

        let result = handler
            .request("GET", "http://example.com/pets/123", None, &AuthCredentials::None, &[], None)
            .await
            .unwrap();

        assert_eq!(result["id"], "123");
        assert_eq!(result["name"], "Fido");
    }

    #[tokio::test]
    async fn test_build_request_bearer_auth() {
        let handler = HttpHandler::mock(|method, url, _query, _body| {
            // The mock doesn't see headers, so we just verify it's called correctly
            assert_eq!(method, "GET");
            assert_eq!(url, "http://example.com/test");
            Ok(serde_json::json!({"ok": true}))
        });

        let auth_config = AuthConfig::Bearer {
            header: "Authorization".to_string(),
            prefix: "Bearer ".to_string(),
        };
        let creds = AuthCredentials::BearerToken("sk-test123".to_string());

        // Test the inject_auth function directly
        let client = reqwest::Client::new();
        let builder = client.get("http://example.com/test");
        let builder = inject_auth(builder, Some(&auth_config), &creds);
        let request = builder.build().unwrap();
        assert_eq!(
            request.headers().get("Authorization").unwrap().to_str().unwrap(),
            "Bearer sk-test123"
        );

        // Also verify mock handler works
        let result = handler
            .request(
                "GET",
                "http://example.com/test",
                Some(&auth_config),
                &creds,
                &[],
                None,
            )
            .await
            .unwrap();
        assert_eq!(result["ok"], true);
    }

    #[tokio::test]
    async fn test_build_request_api_key_auth() {
        let auth_config = AuthConfig::ApiKey {
            header: "X-API-Key".to_string(),
        };
        let creds = AuthCredentials::ApiKey("my-secret-key".to_string());

        let client = reqwest::Client::new();
        let builder = client.get("http://example.com/test");
        let builder = inject_auth(builder, Some(&auth_config), &creds);
        let request = builder.build().unwrap();
        assert_eq!(
            request.headers().get("X-API-Key").unwrap().to_str().unwrap(),
            "my-secret-key"
        );
    }

    fn test_manifest_with_api(name: &str) -> Manifest {
        Manifest {
            apis: vec![ApiConfig {
                name: name.to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: None,
                auth: None,
            }],
            functions: vec![],
            schemas: vec![],
        }
    }

    #[test]
    fn test_load_bearer_from_env() {
        let manifest = test_manifest_with_api("myapi");
        std::env::set_var("MYAPI_BEARER_TOKEN", "sk-test-token");
        let auth = load_auth_from_env(&manifest);
        std::env::remove_var("MYAPI_BEARER_TOKEN");

        assert!(auth.contains_key("myapi"));
        match &auth["myapi"] {
            AuthCredentials::BearerToken(token) => assert_eq!(token, "sk-test-token"),
            other => panic!("Expected BearerToken, got {:?}", other),
        }
    }

    #[test]
    fn test_load_api_key_from_env() {
        let manifest = test_manifest_with_api("testapi");
        // Ensure bearer token is not set (higher precedence)
        std::env::remove_var("TESTAPI_BEARER_TOKEN");
        std::env::set_var("TESTAPI_API_KEY", "key-abc123");
        let auth = load_auth_from_env(&manifest);
        std::env::remove_var("TESTAPI_API_KEY");

        assert!(auth.contains_key("testapi"));
        match &auth["testapi"] {
            AuthCredentials::ApiKey(key) => assert_eq!(key, "key-abc123"),
            other => panic!("Expected ApiKey, got {:?}", other),
        }
    }

    #[test]
    fn test_load_no_env_returns_empty() {
        let manifest = test_manifest_with_api("noenv");
        // Make sure no env vars are set
        std::env::remove_var("NOENV_BEARER_TOKEN");
        std::env::remove_var("NOENV_API_KEY");
        std::env::remove_var("NOENV_BASIC_USER");
        std::env::remove_var("NOENV_BASIC_PASS");

        let auth = load_auth_from_env(&manifest);
        assert!(auth.is_empty());
    }
}
