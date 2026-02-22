use crate::runtime::http::AuthCredentials;

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
}
