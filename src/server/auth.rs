use std::sync::Arc;

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use tokio::sync::RwLock;

use crate::runtime::http::{AuthCredentials, AuthCredentialsMap};

/// Configuration for MCP-level JWT authentication on the HTTP transport.
#[derive(Clone, Debug)]
pub struct McpAuthConfig {
    pub authority: String,
    pub audience: String,
    pub jwks_uri_override: Option<String>,
}

impl McpAuthConfig {
    pub fn from_env() -> Option<Self> {
        let authority = std::env::var("MCP_AUTH_AUTHORITY").ok()?;
        let audience = std::env::var("MCP_AUTH_AUDIENCE").ok()?;
        let jwks_uri_override = std::env::var("MCP_AUTH_JWKS_URI").ok();
        Some(Self {
            authority,
            audience,
            jwks_uri_override,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing Authorization header")]
    MissingHeader,
    #[error("invalid Authorization header")]
    InvalidHeader,
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("JWKS fetch error: {0}")]
    JwksFetchError(String),
}

#[derive(Clone, Debug)]
pub struct AuthContext {
    pub subject: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MetaAuthEntry {
    Bearer { token: String },
    ApiKey { key: String },
    Basic { username: String, password: String },
}

impl From<MetaAuthEntry> for AuthCredentials {
    fn from(entry: MetaAuthEntry) -> Self {
        match entry {
            MetaAuthEntry::Bearer { token } => Self::BearerToken(token),
            MetaAuthEntry::ApiKey { key } => Self::ApiKey(key),
            MetaAuthEntry::Basic { username, password } => Self::Basic { username, password },
        }
    }
}

/// Parse the `_meta.auth` JSON value into an `AuthCredentialsMap`.
///
/// Each key in the JSON object is an API name, and each value is a
/// `MetaAuthEntry` (tagged by `"type"`). Entries that fail to deserialize
/// are silently skipped.
pub fn parse_meta_auth(auth_value: &serde_json::Value) -> AuthCredentialsMap {
    let mut map = AuthCredentialsMap::new();
    if let Some(obj) = auth_value.as_object() {
        for (api_name, entry_value) in obj {
            if let Ok(entry) = serde_json::from_value::<MetaAuthEntry>(entry_value.clone()) {
                map.insert(api_name.clone(), entry.into());
            }
        }
    }
    map
}

/// Merge environment-loaded credentials with per-request `_meta.auth` credentials.
///
/// Meta credentials take precedence: if the same API name appears in both maps,
/// the meta value wins.
pub fn merge_credentials(
    env_creds: &AuthCredentialsMap,
    meta_creds: &AuthCredentialsMap,
) -> AuthCredentialsMap {
    let mut merged = env_creds.clone();
    for (api_name, creds) in meta_creds {
        merged.insert(api_name.clone(), creds.clone());
    }
    merged
}

// ---------------------------------------------------------------------------
// JWT validation
// ---------------------------------------------------------------------------

/// Internal claims structure for deserializing JWTs.
///
/// The `aud` field uses `serde_json::Value` because the JWT spec allows both a
/// single string and an array of strings. The `jsonwebtoken` crate handles
/// audience *validation* separately via `Validation::set_audience`, so we only
/// need `aud` here to prevent deserialization failures.
#[derive(Debug, serde::Deserialize)]
struct JwtClaims {
    sub: String,
    #[allow(dead_code)]
    iss: String,
    #[allow(dead_code)]
    #[serde(default)]
    aud: serde_json::Value,
    #[allow(dead_code)]
    exp: u64,
}

/// Validate a JWT against a known key, algorithm, issuer, and audience.
///
/// Returns an [`AuthContext`] on success, or an [`AuthError`] if validation
/// fails for any reason (expired, wrong issuer, wrong audience, bad signature, etc.).
pub fn validate_jwt_with_key(
    token: &str,
    key: &DecodingKey,
    algorithm: Algorithm,
    expected_issuer: &str,
    expected_audience: &str,
) -> Result<AuthContext, AuthError> {
    let mut validation = Validation::new(algorithm);
    validation.set_audience(&[expected_audience]);
    validation.set_issuer(&[expected_issuer]);
    validation.set_required_spec_claims(&["exp", "sub", "iss", "aud"]);

    let token_data = decode::<JwtClaims>(token, key, &validation)
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

    Ok(AuthContext {
        subject: token_data.claims.sub,
    })
}

// ---------------------------------------------------------------------------
// JWKS fetching and caching
// ---------------------------------------------------------------------------

/// OIDC discovery document (minimal subset).
#[derive(Debug, serde::Deserialize)]
struct OidcDiscovery {
    jwks_uri: String,
}

/// Validates JWTs by fetching and caching the issuer's JWKS.
pub struct JwtValidator {
    config: McpAuthConfig,
    jwks: Arc<RwLock<Option<JwkSet>>>,
    http_client: reqwest::Client,
}

impl JwtValidator {
    pub fn new(config: McpAuthConfig) -> Self {
        Self {
            config,
            jwks: Arc::new(RwLock::new(None)),
            http_client: reqwest::Client::new(),
        }
    }

    /// Resolve the JWKS URI, either from config override or OIDC discovery.
    async fn resolve_jwks_uri(&self) -> Result<String, AuthError> {
        if let Some(ref uri) = self.config.jwks_uri_override {
            return Ok(uri.clone());
        }

        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            self.config.authority.trim_end_matches('/')
        );

        let resp = self
            .http_client
            .get(&discovery_url)
            .send()
            .await
            .map_err(|e| {
                AuthError::JwksFetchError(format!("OIDC discovery request failed: {e}"))
            })?;

        if !resp.status().is_success() {
            return Err(AuthError::JwksFetchError(format!(
                "OIDC discovery returned status {}",
                resp.status()
            )));
        }

        let doc: OidcDiscovery = resp.json().await.map_err(|e| {
            AuthError::JwksFetchError(format!("failed to parse OIDC discovery: {e}"))
        })?;

        Ok(doc.jwks_uri)
    }

    /// Return the cached JWKS, fetching it first if the cache is empty.
    async fn get_jwks(&self) -> Result<JwkSet, AuthError> {
        {
            let cache = self.jwks.read().await;
            if let Some(ref jwks) = *cache {
                return Ok(jwks.clone());
            }
        }
        self.refresh_jwks().await
    }

    /// Fetch the JWKS from the authority and update the cache.
    async fn refresh_jwks(&self) -> Result<JwkSet, AuthError> {
        let uri = self.resolve_jwks_uri().await?;

        let resp = self
            .http_client
            .get(&uri)
            .send()
            .await
            .map_err(|e| AuthError::JwksFetchError(format!("JWKS fetch failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AuthError::JwksFetchError(format!(
                "JWKS endpoint returned status {}",
                resp.status()
            )));
        }

        let jwks: JwkSet = resp
            .json()
            .await
            .map_err(|e| AuthError::JwksFetchError(format!("failed to parse JWKS: {e}")))?;

        {
            let mut cache = self.jwks.write().await;
            *cache = Some(jwks.clone());
        }

        Ok(jwks)
    }

    /// Validate a JWT token using the cached (or freshly fetched) JWKS.
    ///
    /// If the `kid` in the token header is not found in the cached JWKS, the
    /// JWKS is refreshed once (key rotation support).
    pub async fn validate(&self, token: &str) -> Result<AuthContext, AuthError> {
        let header = decode_header(token)
            .map_err(|e| AuthError::InvalidToken(format!("bad header: {e}")))?;

        let kid = header
            .kid
            .as_deref()
            .ok_or_else(|| AuthError::InvalidToken("token has no kid".to_string()))?;

        let algorithm = header.alg;

        // Try cached JWKS first
        let jwks = self.get_jwks().await?;

        let jwk = if let Some(jwk) = jwks.find(kid) {
            jwk.clone()
        } else {
            // Key rotation: refresh and try once more
            let refreshed = self.refresh_jwks().await?;
            refreshed.find(kid).cloned().ok_or_else(|| {
                AuthError::InvalidToken(format!("no matching JWK for kid '{kid}'"))
            })?
        };

        let decoding_key = DecodingKey::from_jwk(&jwk)
            .map_err(|e| AuthError::InvalidToken(format!("invalid JWK: {e}")))?;

        validate_jwt_with_key(
            token,
            &decoding_key,
            algorithm,
            &self.config.authority,
            &self.config.audience,
        )
    }
}

// ---------------------------------------------------------------------------
// Tower auth middleware
// ---------------------------------------------------------------------------

/// Extract the bearer token from an Authorization header value.
///
/// Expects the format `Bearer <token>`. Returns `AuthError::MissingHeader` if
/// the header value is empty, or `AuthError::InvalidHeader` if the scheme is
/// not `Bearer` or the token is empty.
pub fn extract_bearer_token(header_value: &str) -> Result<&str, AuthError> {
    if header_value.is_empty() {
        return Err(AuthError::MissingHeader);
    }
    let token = header_value
        .strip_prefix("Bearer ")
        .ok_or(AuthError::InvalidHeader)?;
    if token.is_empty() {
        return Err(AuthError::InvalidHeader);
    }
    Ok(token)
}

/// Build the `WWW-Authenticate` header value per RFC 9728.
pub fn www_authenticate_value(config: &McpAuthConfig) -> String {
    format!(
        "Bearer realm=\"{}\", resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
        config.audience, config.audience
    )
}

/// Build a 401 Unauthorized response with the proper `WWW-Authenticate` header.
pub fn unauthorized_response(config: &McpAuthConfig) -> axum::response::Response<axum::body::Body> {
    axum::response::Response::builder()
        .status(axum::http::StatusCode::UNAUTHORIZED)
        .header("WWW-Authenticate", www_authenticate_value(config))
        .body(axum::body::Body::from("Unauthorized"))
        .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::from("Unauthorized")))
}

/// Auth middleware for `axum::middleware::from_fn_with_state`.
///
/// State is `(Arc<JwtValidator>, McpAuthConfig)`. On success the [`AuthContext`]
/// is inserted into the request extensions so downstream handlers can access it.
pub async fn auth_middleware(
    axum::extract::State((validator, config)): axum::extract::State<(
        Arc<JwtValidator>,
        McpAuthConfig,
    )>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response<axum::body::Body> {
    let auth_header = match request.headers().get("authorization") {
        Some(h) => match h.to_str() {
            Ok(s) => s,
            Err(_) => return unauthorized_response(&config),
        },
        None => return unauthorized_response(&config),
    };

    let Ok(token) = extract_bearer_token(auth_header) else {
        return unauthorized_response(&config);
    };

    match validator.validate(token).await {
        Ok(auth_context) => {
            request.extensions_mut().insert(auth_context);
            next.run(request).await
        }
        Err(_) => unauthorized_response(&config),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, unsafe_code)]
    use super::*;

    #[test]
    fn test_mcp_auth_config_from_env() {
        // SAFETY: test-only env manipulation; tests run serially for env vars
        unsafe {
            std::env::set_var("MCP_AUTH_AUTHORITY", "https://auth.example.com");
            std::env::set_var("MCP_AUTH_AUDIENCE", "https://mcp.example.com");
            std::env::remove_var("MCP_AUTH_JWKS_URI");
        }

        let config = McpAuthConfig::from_env();
        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(config.authority, "https://auth.example.com");
        assert_eq!(config.audience, "https://mcp.example.com");
        assert!(config.jwks_uri_override.is_none());

        // SAFETY: test-only env cleanup
        unsafe {
            std::env::remove_var("MCP_AUTH_AUTHORITY");
            std::env::remove_var("MCP_AUTH_AUDIENCE");
        }
    }

    #[test]
    fn test_mcp_auth_config_from_env_with_jwks_override() {
        // SAFETY: test-only env manipulation; tests run serially for env vars
        unsafe {
            std::env::set_var("MCP_AUTH_AUTHORITY", "https://auth.example.com");
            std::env::set_var("MCP_AUTH_AUDIENCE", "https://mcp.example.com");
            std::env::set_var("MCP_AUTH_JWKS_URI", "https://auth.example.com/custom/jwks");
        }

        let config = McpAuthConfig::from_env();
        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(
            config.jwks_uri_override.as_deref(),
            Some("https://auth.example.com/custom/jwks")
        );

        // SAFETY: test-only env cleanup
        unsafe {
            std::env::remove_var("MCP_AUTH_AUTHORITY");
            std::env::remove_var("MCP_AUTH_AUDIENCE");
            std::env::remove_var("MCP_AUTH_JWKS_URI");
        }
    }

    #[test]
    fn test_mcp_auth_config_from_env_missing() {
        // SAFETY: test-only env manipulation; tests run serially for env vars
        unsafe {
            std::env::remove_var("MCP_AUTH_AUTHORITY");
            std::env::remove_var("MCP_AUTH_AUDIENCE");
        }

        let config = McpAuthConfig::from_env();
        assert!(config.is_none());
    }

    #[test]
    fn test_parse_meta_auth_bearer() {
        let meta_json = serde_json::json!({
            "petstore": { "type": "bearer", "token": "sk-secret" }
        });
        let result = parse_meta_auth(&meta_json);
        assert_eq!(result.len(), 1);
        match &result["petstore"] {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "sk-secret"),
            other => panic!("expected BearerToken, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_meta_auth_multiple() {
        let meta_json = serde_json::json!({
            "petstore": { "type": "bearer", "token": "sk-pet" },
            "billing": { "type": "api_key", "key": "billing-key" },
            "legacy": { "type": "basic", "username": "user", "password": "pass" }
        });
        let result = parse_meta_auth(&meta_json);
        assert_eq!(result.len(), 3);
        assert!(matches!(
            &result["petstore"],
            AuthCredentials::BearerToken(_)
        ));
        assert!(matches!(&result["billing"], AuthCredentials::ApiKey(_)));
        assert!(matches!(&result["legacy"], AuthCredentials::Basic { .. }));
    }

    #[test]
    fn test_parse_meta_auth_invalid_entry_skipped() {
        let meta_json = serde_json::json!({
            "good": { "type": "bearer", "token": "sk-ok" },
            "bad": { "type": "unknown_type" }
        });
        let result = parse_meta_auth(&meta_json);
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("good"));
    }

    #[test]
    fn test_merge_credentials_meta_overrides_env() {
        let mut env_creds = AuthCredentialsMap::new();
        env_creds.insert(
            "petstore".to_string(),
            AuthCredentials::BearerToken("env-token".to_string()),
        );
        env_creds.insert(
            "billing".to_string(),
            AuthCredentials::ApiKey("env-key".to_string()),
        );

        let mut meta_creds = AuthCredentialsMap::new();
        meta_creds.insert(
            "petstore".to_string(),
            AuthCredentials::BearerToken("meta-token".to_string()),
        );

        let merged = merge_credentials(&env_creds, &meta_creds);

        match &merged["petstore"] {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "meta-token"),
            other => panic!("expected meta-token, got {:?}", other),
        }
        assert!(matches!(&merged["billing"], AuthCredentials::ApiKey(_)));
    }

    // ----- Task 4: JWT validation tests -----

    use jsonwebtoken::{Algorithm as JwtAlgorithm, EncodingKey, Header, encode};

    #[derive(serde::Serialize)]
    struct TestClaims {
        sub: String,
        iss: String,
        aud: String,
        exp: u64,
    }

    #[test]
    fn test_validate_jwt_claims() {
        let secret = b"test-secret-key-that-is-long-enough-for-hs256";
        let claims = TestClaims {
            sub: "user-123".to_string(),
            iss: "https://auth.example.com".to_string(),
            aud: "https://mcp.example.com".to_string(),
            exp: (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs())
                + 3600,
        };
        let token = encode(
            &Header::new(JwtAlgorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        let result = validate_jwt_with_key(
            &token,
            &jsonwebtoken::DecodingKey::from_secret(secret),
            Algorithm::HS256,
            "https://auth.example.com",
            "https://mcp.example.com",
        );
        assert!(result.is_ok());
        let ctx = result.unwrap();
        assert_eq!(ctx.subject, "user-123");
    }

    #[test]
    fn test_validate_jwt_expired() {
        let secret = b"test-secret-key-that-is-long-enough-for-hs256";
        let claims = TestClaims {
            sub: "user-123".to_string(),
            iss: "https://auth.example.com".to_string(),
            aud: "https://mcp.example.com".to_string(),
            exp: 1000,
        };
        let token = encode(
            &Header::new(JwtAlgorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        let result = validate_jwt_with_key(
            &token,
            &jsonwebtoken::DecodingKey::from_secret(secret),
            Algorithm::HS256,
            "https://auth.example.com",
            "https://mcp.example.com",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_jwt_wrong_audience() {
        let secret = b"test-secret-key-that-is-long-enough-for-hs256";
        let claims = TestClaims {
            sub: "user-123".to_string(),
            iss: "https://auth.example.com".to_string(),
            aud: "https://wrong-audience.com".to_string(),
            exp: (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs())
                + 3600,
        };
        let token = encode(
            &Header::new(JwtAlgorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        let result = validate_jwt_with_key(
            &token,
            &jsonwebtoken::DecodingKey::from_secret(secret),
            Algorithm::HS256,
            "https://auth.example.com",
            "https://mcp.example.com",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_jwt_wrong_issuer() {
        let secret = b"test-secret-key-that-is-long-enough-for-hs256";
        let claims = TestClaims {
            sub: "user-123".to_string(),
            iss: "https://wrong-issuer.com".to_string(),
            aud: "https://mcp.example.com".to_string(),
            exp: (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs())
                + 3600,
        };
        let token = encode(
            &Header::new(JwtAlgorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        let result = validate_jwt_with_key(
            &token,
            &jsonwebtoken::DecodingKey::from_secret(secret),
            Algorithm::HS256,
            "https://auth.example.com",
            "https://mcp.example.com",
        );
        assert!(result.is_err());
    }

    // ----- Task 5: Tower auth middleware tests -----

    #[test]
    fn test_extract_bearer_token_valid() {
        let result = extract_bearer_token("Bearer eyJhbGciOiJIUzI1NiJ9.test.sig");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "eyJhbGciOiJIUzI1NiJ9.test.sig");
    }

    #[test]
    fn test_extract_bearer_token_missing() {
        let result = extract_bearer_token("");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_bearer_token_wrong_scheme() {
        let result = extract_bearer_token("Basic dXNlcjpwYXNz");
        assert!(result.is_err());
    }

    #[test]
    fn test_www_authenticate_header() {
        let config = McpAuthConfig {
            authority: "https://auth.example.com".to_string(),
            audience: "https://mcp.example.com".to_string(),
            jwks_uri_override: None,
        };
        let header = www_authenticate_value(&config);
        assert!(header.contains("Bearer"));
        assert!(header.contains("https://mcp.example.com"));
    }
}
