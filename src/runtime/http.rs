use std::collections::HashMap;
use std::sync::Arc;

use crate::codegen::manifest::AuthConfig;

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

/// Mock function signature: (method, url, `query_params`, body) -> `Result<serde_json::Value>`
type MockFn = Arc<
    dyn Fn(
            &str,
            &str,
            &[(String, String)],
            Option<&serde_json::Value>,
        ) -> anyhow::Result<serde_json::Value>
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

impl Default for HttpHandler {
    fn default() -> Self {
        Self::new()
    }
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
        F: Fn(
                &str,
                &str,
                &[(String, String)],
                Option<&serde_json::Value>,
            ) -> anyhow::Result<serde_json::Value>
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
                let req_method = method
                    .parse::<reqwest::Method>()
                    .map_err(|e| anyhow::anyhow!("invalid HTTP method '{method}': {e}"))?;

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

/// Inject authentication into the request builder based on config + credentials.
fn inject_auth(
    mut builder: reqwest::RequestBuilder,
    auth_config: Option<&AuthConfig>,
    credentials: &AuthCredentials,
) -> reqwest::RequestBuilder {
    match (auth_config, credentials) {
        (Some(AuthConfig::Bearer { header, prefix }), AuthCredentials::BearerToken(token)) => {
            let value = format!("{prefix}{token}");
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[tokio::test]
    async fn test_mock_handler_returns_response() {
        let handler = HttpHandler::mock(|_method, _url, _query, _body| {
            Ok(serde_json::json!({"id": "123", "name": "Fido"}))
        });

        let result = handler
            .request(
                "GET",
                "http://example.com/pets/123",
                None,
                &AuthCredentials::None,
                &[],
                None,
            )
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
            request
                .headers()
                .get("Authorization")
                .unwrap()
                .to_str()
                .unwrap(),
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
            request
                .headers()
                .get("X-API-Key")
                .unwrap()
                .to_str()
                .unwrap(),
            "my-secret-key"
        );
    }
}
