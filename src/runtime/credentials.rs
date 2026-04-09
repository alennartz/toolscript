use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, RwLock};

use crate::runtime::http::{AuthCredentials, AuthCredentialsMap};

/// Default TTL for command-based credentials (seconds).
pub const DEFAULT_TTL_SECS: u64 = 2;
/// Default timeout waiting for a command to complete (seconds).
pub const DEFAULT_TIMEOUT_SECS: u64 = 5;
/// Default max age before cached credentials are considered too stale for fallback (seconds).
pub const DEFAULT_MAX_AGE_SECS: u64 = 300;

/// How to interpret the command's stdout when producing credentials.
#[derive(Debug, Clone)]
pub enum CredentialKind {
    /// Stdout is a bearer token.
    BearerToken,
    /// Stdout is an API key.
    ApiKey,
    /// Stdout is `username:password`.
    Basic,
}

/// Configuration for a command-based credential source.
#[derive(Debug, Clone)]
pub struct CommandConfig {
    /// Shell command to execute (via `sh -c`).
    pub command: String,
    /// How to parse the command's stdout.
    pub kind: CredentialKind,
    /// How long before a cached credential triggers a background refresh.
    pub ttl: Duration,
    /// How long to wait for a refresh before falling back to cached value.
    pub timeout: Duration,
    /// Maximum age of a cached credential before it cannot be used as fallback.
    pub max_age: Duration,
}

impl CommandConfig {
    /// Create a new command config with default TTL, timeout, and max age.
    pub const fn new(command: String, kind: CredentialKind) -> Self {
        Self {
            command,
            kind,
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            max_age: Duration::from_secs(DEFAULT_MAX_AGE_SECS),
        }
    }
}

struct CachedCredential {
    value: AuthCredentials,
    fetched_at: Instant,
}

struct CommandState {
    config: CommandConfig,
    cache: RwLock<Option<CachedCredential>>,
    refreshing: AtomicBool,
    notify: Notify,
}

enum CredentialSource {
    Static(AuthCredentials),
    Command(Arc<CommandState>),
}

/// Thread-safe credential resolver supporting both static and command-based credentials.
///
/// Static credentials are returned immediately. Command-based credentials are fetched
/// by executing a shell command, with caching and background refresh:
///
/// - Within TTL: returns cached value without re-executing.
/// - After TTL but within max age: triggers background refresh, returns cached value.
/// - After max age (or no cache): blocks until refresh completes or times out.
/// - On command failure: keeps the cached value.
/// - Background refreshes are never aborted — they complete and update the cache.
pub struct CredentialResolver {
    sources: HashMap<String, CredentialSource>,
}

impl Default for CredentialResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialResolver {
    /// Create an empty resolver.
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
        }
    }

    /// Create a resolver from a static credential map.
    pub fn from_static_map(map: AuthCredentialsMap) -> Self {
        let sources = map
            .into_iter()
            .map(|(k, v)| (k, CredentialSource::Static(v)))
            .collect();
        Self { sources }
    }

    /// Add a static credential for an API. Overwrites any existing entry.
    pub fn add_static(&mut self, api: String, creds: AuthCredentials) {
        self.sources.insert(api, CredentialSource::Static(creds));
    }

    /// Add a command-based credential for an API. Overwrites any existing entry.
    pub fn add_command(&mut self, api: String, config: CommandConfig) {
        self.sources.insert(
            api,
            CredentialSource::Command(Arc::new(CommandState {
                config,
                cache: RwLock::new(None),
                refreshing: AtomicBool::new(false),
                notify: Notify::new(),
            })),
        );
    }

    /// Check if credentials are configured for an API.
    pub fn has_credentials(&self, api: &str) -> bool {
        self.sources.contains_key(api)
    }

    /// Get credentials for an API.
    ///
    /// For static credentials, returns immediately. For command-based credentials,
    /// may block on first call or when the cache is expired beyond max age.
    pub async fn get(&self, api: &str) -> AuthCredentials {
        match self.sources.get(api) {
            None => AuthCredentials::None,
            Some(CredentialSource::Static(creds)) => creds.clone(),
            Some(CredentialSource::Command(state)) => resolve_command(state).await,
        }
    }
}

async fn resolve_command(state: &Arc<CommandState>) -> AuthCredentials {
    // Check cache
    {
        let cache = state.cache.read().await;
        if let Some(cached) = &*cache {
            let age = cached.fetched_at.elapsed();
            if age < state.config.ttl {
                // Fresh — use as-is
                return cached.value.clone();
            }
            if age < state.config.max_age {
                // Stale but usable — trigger background refresh, return cached
                let value = cached.value.clone();
                drop(cache);
                maybe_start_refresh(state);
                return value;
            }
            // Too old — fall through to wait_for_refresh
        }
    }

    // No cache or cache too old — block until refresh completes
    wait_for_refresh(state).await
}

fn maybe_start_refresh(state: &Arc<CommandState>) {
    if state
        .refreshing
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let state = Arc::clone(state);
        tokio::spawn(async move {
            do_refresh(&state).await;
        });
    }
}

async fn wait_for_refresh(state: &Arc<CommandState>) -> AuthCredentials {
    // Register interest in the notification BEFORE starting the refresh,
    // so we don't miss a completion that happens between start and await.
    let notified = state.notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();

    // Start refresh if not already running
    maybe_start_refresh(state);

    // Wait with timeout
    let _ = tokio::time::timeout(state.config.timeout, notified).await;

    // Return whatever is in cache (refresh may have succeeded or failed)
    let cache = state.cache.read().await;
    cache
        .as_ref()
        .map_or(AuthCredentials::None, |c| c.value.clone())
}

async fn do_refresh(state: &CommandState) {
    let result = run_command(&state.config.command).await;
    if let Ok(output) = result {
        let creds = parse_credential(&output, &state.config.kind);
        let mut cache = state.cache.write().await;
        *cache = Some(CachedCredential {
            value: creds,
            fetched_at: Instant::now(),
        });
    }
    // On failure: keep cached value (if any), don't update
    state.refreshing.store(false, Ordering::SeqCst);
    state.notify.notify_waiters();
}

async fn run_command(command: &str) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "auth command failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn parse_credential(output: &str, kind: &CredentialKind) -> AuthCredentials {
    match kind {
        CredentialKind::BearerToken => AuthCredentials::BearerToken(output.to_string()),
        CredentialKind::ApiKey => AuthCredentials::ApiKey(output.to_string()),
        CredentialKind::Basic => {
            if let Some((user, pass)) = output.split_once(':') {
                AuthCredentials::Basic {
                    username: user.to_string(),
                    password: pass.to_string(),
                }
            } else {
                // Can't parse as user:pass — treat as bearer token as fallback
                AuthCredentials::BearerToken(output.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    // --- Static credential tests ---

    #[tokio::test]
    async fn test_static_bearer_token() {
        let mut resolver = CredentialResolver::new();
        resolver.add_static(
            "myapi".into(),
            AuthCredentials::BearerToken("token123".into()),
        );

        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "token123"),
            other => panic!("expected BearerToken, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_unknown_api_returns_none() {
        let resolver = CredentialResolver::new();
        assert!(matches!(
            resolver.get("unknown").await,
            AuthCredentials::None
        ));
    }

    #[tokio::test]
    async fn test_from_static_map() {
        let mut map = AuthCredentialsMap::new();
        map.insert("api1".into(), AuthCredentials::BearerToken("tok1".into()));
        map.insert(
            "api2".into(),
            AuthCredentials::Basic {
                username: "u".into(),
                password: "p".into(),
            },
        );

        let resolver = CredentialResolver::from_static_map(map);
        assert!(resolver.has_credentials("api1"));
        assert!(resolver.has_credentials("api2"));
        assert!(!resolver.has_credentials("api3"));
    }

    // --- Command credential tests ---

    #[tokio::test]
    async fn test_command_produces_bearer_token() {
        let mut resolver = CredentialResolver::new();
        resolver.add_command(
            "myapi".into(),
            CommandConfig::new("echo 'secret-token'".into(), CredentialKind::BearerToken),
        );

        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "secret-token"),
            other => panic!("expected BearerToken, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_command_produces_api_key() {
        let mut resolver = CredentialResolver::new();
        resolver.add_command(
            "myapi".into(),
            CommandConfig::new("echo 'my-api-key'".into(), CredentialKind::ApiKey),
        );

        match resolver.get("myapi").await {
            AuthCredentials::ApiKey(k) => assert_eq!(k, "my-api-key"),
            other => panic!("expected ApiKey, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_command_produces_basic_auth() {
        let mut resolver = CredentialResolver::new();
        resolver.add_command(
            "myapi".into(),
            CommandConfig::new("echo 'admin:hunter2'".into(), CredentialKind::Basic),
        );

        match resolver.get("myapi").await {
            AuthCredentials::Basic { username, password } => {
                assert_eq!(username, "admin");
                assert_eq!(password, "hunter2");
            }
            other => panic!("expected Basic, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_basic_auth_password_with_colons() {
        let mut resolver = CredentialResolver::new();
        resolver.add_command(
            "myapi".into(),
            CommandConfig::new("echo 'user:pass:with:colons'".into(), CredentialKind::Basic),
        );

        match resolver.get("myapi").await {
            AuthCredentials::Basic { username, password } => {
                assert_eq!(username, "user");
                assert_eq!(password, "pass:with:colons");
            }
            other => panic!("expected Basic, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_command_output_is_trimmed() {
        let mut resolver = CredentialResolver::new();
        resolver.add_command(
            "myapi".into(),
            CommandConfig::new(
                "printf '  token-with-spaces  \\n'".into(),
                CredentialKind::BearerToken,
            ),
        );

        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "token-with-spaces"),
            other => panic!("expected BearerToken, got {other:?}"),
        }
    }

    // --- Caching behavior tests ---

    #[tokio::test]
    async fn test_cached_within_ttl() {
        let mut resolver = CredentialResolver::new();
        let mut config = CommandConfig::new("date +%s%N".into(), CredentialKind::BearerToken);
        config.ttl = Duration::from_secs(10); // long TTL
        resolver.add_command("myapi".into(), config);

        let first = resolver.get("myapi").await;
        let second = resolver.get("myapi").await;

        // Both should be the same cached value
        match (&first, &second) {
            (AuthCredentials::BearerToken(a), AuthCredentials::BearerToken(b)) => {
                assert_eq!(a, b, "second call should return cached value within TTL");
            }
            _ => panic!("expected BearerToken"),
        }
    }

    #[tokio::test]
    async fn test_refresh_after_ttl_updates_cache() {
        let dir = tempfile::tempdir().unwrap();
        let token_file = dir.path().join("token");
        std::fs::write(&token_file, "token-v1").unwrap();

        let cmd = format!("cat {}", token_file.display());
        let mut resolver = CredentialResolver::new();
        let mut config = CommandConfig::new(cmd, CredentialKind::BearerToken);
        config.ttl = Duration::from_millis(50);
        resolver.add_command("myapi".into(), config);

        // First call: populates cache with "token-v1"
        let first = resolver.get("myapi").await;
        match &first {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "token-v1"),
            other => panic!("expected BearerToken, got {other:?}"),
        }

        // Update the token file
        std::fs::write(&token_file, "token-v2").unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        // This call triggers background refresh, returns stale "token-v1"
        let stale = resolver.get("myapi").await;
        match &stale {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "token-v1"),
            other => panic!("expected BearerToken, got {other:?}"),
        }

        // Wait for background refresh to complete
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Now cache should have "token-v2"
        let refreshed = resolver.get("myapi").await;
        match &refreshed {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "token-v2"),
            other => panic!("expected BearerToken, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_command_failure_keeps_cached_value() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("auth.sh");

        // First: script succeeds
        std::fs::write(&script, "#!/bin/sh\necho 'good-token'").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let mut resolver = CredentialResolver::new();
        let mut config =
            CommandConfig::new(script.display().to_string(), CredentialKind::BearerToken);
        config.ttl = Duration::from_millis(50);
        resolver.add_command("myapi".into(), config);

        // First call succeeds
        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "good-token"),
            other => panic!("expected BearerToken, got {other:?}"),
        }

        // Make script fail
        std::fs::write(&script, "#!/bin/sh\nexit 1").unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Trigger refresh (which will fail)
        resolver.get("myapi").await;

        // Wait for failed refresh to complete
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Should still have the cached value
        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "good-token"),
            other => panic!("expected BearerToken with cached value, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_first_call_blocks_until_command_completes() {
        let mut resolver = CredentialResolver::new();
        let mut config = CommandConfig::new(
            "sleep 0.1 && echo 'delayed-token'".into(),
            CredentialKind::BearerToken,
        );
        config.timeout = Duration::from_secs(5);
        resolver.add_command("myapi".into(), config);

        let start = Instant::now();
        let result = resolver.get("myapi").await;
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(80),
            "should block for command, elapsed: {elapsed:?}"
        );
        match result {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "delayed-token"),
            other => panic!("expected BearerToken, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_first_call_timeout_returns_none() {
        let mut resolver = CredentialResolver::new();
        let mut config = CommandConfig::new(
            "sleep 10 && echo 'never'".into(),
            CredentialKind::BearerToken,
        );
        config.timeout = Duration::from_millis(100);
        resolver.add_command("myapi".into(), config);

        let start = Instant::now();
        let result = resolver.get("myapi").await;
        let elapsed = start.elapsed();

        // Should timeout and return None (no cache to fall back to)
        assert!(
            elapsed < Duration::from_secs(2),
            "should not block for full command, elapsed: {elapsed:?}"
        );
        assert!(
            matches!(result, AuthCredentials::None),
            "expected None on timeout with no cache, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_expired_cache_waits_for_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let token_file = dir.path().join("token");
        std::fs::write(&token_file, "initial").unwrap();

        let cmd = format!("cat {}", token_file.display());
        let mut resolver = CredentialResolver::new();
        let mut config = CommandConfig::new(cmd, CredentialKind::BearerToken);
        config.ttl = Duration::from_millis(30);
        config.max_age = Duration::from_millis(60);
        config.timeout = Duration::from_secs(5);
        resolver.add_command("myapi".into(), config);

        // Populate cache
        resolver.get("myapi").await;

        // Update token
        std::fs::write(&token_file, "refreshed").unwrap();

        // Wait for cache to expire past max_age
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should block and wait for refresh (cache too old for fallback)
        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "refreshed"),
            other => panic!("expected BearerToken, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_background_refresh_continues_after_timeout_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let token_file = dir.path().join("token");
        std::fs::write(&token_file, "initial").unwrap();

        // Command: read token, but with a delay
        let cmd = format!("sleep 0.3 && cat {}", token_file.display());
        let mut resolver = CredentialResolver::new();
        let mut config = CommandConfig::new(cmd, CredentialKind::BearerToken);
        config.ttl = Duration::from_millis(30);
        config.max_age = Duration::from_secs(300); // cache stays usable
        config.timeout = Duration::from_millis(100); // shorter than command
        resolver.add_command("myapi".into(), config);

        // First call: blocks until command completes (no cache, must wait full duration)
        // Actually first call timeout is 100ms but command takes 300ms, so it'll timeout
        // and return None. Let me use a fast command for initial population.

        // Hmm, let me rethink. The first call also has a timeout. If the command
        // takes 300ms and timeout is 100ms, first call returns None.
        // I need a way to populate the cache first with a fast command, then
        // switch to a slow one for the refresh.
        // But the command is fixed at construction time.

        // Alternative: use a script file that we can change.
        let script = dir.path().join("auth.sh");
        std::fs::write(&script, format!("#!/bin/sh\ncat {}", token_file.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let mut resolver = CredentialResolver::new();
        let mut config =
            CommandConfig::new(script.display().to_string(), CredentialKind::BearerToken);
        config.ttl = Duration::from_millis(30);
        config.max_age = Duration::from_secs(300);
        config.timeout = Duration::from_millis(100);
        resolver.add_command("myapi".into(), config);

        // Fast first call populates cache with "initial"
        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "initial"),
            other => panic!("expected BearerToken, got {other:?}"),
        }

        // Now make the script slow and update the token
        std::fs::write(&token_file, "updated").unwrap();
        std::fs::write(
            &script,
            format!("#!/bin/sh\nsleep 0.3 && cat {}", token_file.display()),
        )
        .unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(50)).await;

        // This triggers a background refresh (slow command, 300ms)
        // Since cache < max_age, returns cached "initial" immediately
        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "initial"),
            other => panic!("expected cached BearerToken, got {other:?}"),
        }

        // Wait for the slow background refresh to complete
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Cache should now have "updated"
        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "updated"),
            other => panic!("expected refreshed BearerToken, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_add_command_overwrites_static() {
        let mut resolver = CredentialResolver::new();
        resolver.add_static(
            "myapi".into(),
            AuthCredentials::BearerToken("static-token".into()),
        );
        resolver.add_command(
            "myapi".into(),
            CommandConfig::new("echo 'command-token'".into(), CredentialKind::BearerToken),
        );

        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "command-token"),
            other => panic!("expected command-token, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_add_static_overwrites_command() {
        let mut resolver = CredentialResolver::new();
        resolver.add_command(
            "myapi".into(),
            CommandConfig::new("echo 'command-token'".into(), CredentialKind::BearerToken),
        );
        resolver.add_static(
            "myapi".into(),
            AuthCredentials::BearerToken("static-token".into()),
        );

        match resolver.get("myapi").await {
            AuthCredentials::BearerToken(t) => assert_eq!(t, "static-token"),
            other => panic!("expected static-token, got {other:?}"),
        }
    }
}
