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
}
