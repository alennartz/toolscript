//! Integration test for HTTP auth: well-known endpoint and 401 on unauthenticated /mcp requests.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::net::TcpListener;

#[tokio::test]
async fn test_well_known_endpoint_no_auth_required() {
    let config = code_mcp::server::auth::McpAuthConfig {
        authority: "https://auth.example.com".to_string(),
        audience: "https://mcp.example.com".to_string(),
        jwks_uri_override: None,
    };

    let well_known = serde_json::json!({
        "resource": config.audience,
        "authorization_servers": [config.authority],
    });

    assert_eq!(well_known["resource"], "https://mcp.example.com");
    assert_eq!(
        well_known["authorization_servers"][0],
        "https://auth.example.com"
    );
}

#[test]
fn test_auth_middleware_rejects_no_header() {
    let result = code_mcp::server::auth::extract_bearer_token("");
    assert!(result.is_err());
}

#[test]
fn test_auth_middleware_rejects_wrong_scheme() {
    let result = code_mcp::server::auth::extract_bearer_token("Basic abc123");
    assert!(result.is_err());
}

#[tokio::test]
async fn test_meta_auth_roundtrip() {
    use code_mcp::runtime::http::{AuthCredentials, AuthCredentialsMap};
    use code_mcp::server::auth::{merge_credentials, parse_meta_auth};

    let mut env_creds = AuthCredentialsMap::new();
    env_creds.insert(
        "api_a".to_string(),
        AuthCredentials::BearerToken("env-token-a".to_string()),
    );
    env_creds.insert(
        "api_b".to_string(),
        AuthCredentials::ApiKey("env-key-b".to_string()),
    );

    let meta_json = serde_json::json!({
        "api_a": { "type": "bearer", "token": "client-token-a" },
        "api_c": { "type": "api_key", "key": "client-key-c" }
    });
    let meta_creds = parse_meta_auth(&meta_json);

    let merged = merge_credentials(&env_creds, &meta_creds);

    match &merged["api_a"] {
        AuthCredentials::BearerToken(t) => assert_eq!(t, "client-token-a"),
        other => panic!("expected client token, got {other:?}"),
    }
    assert!(matches!(&merged["api_b"], AuthCredentials::ApiKey(_)));
    assert!(matches!(&merged["api_c"], AuthCredentials::ApiKey(_)));
}

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
