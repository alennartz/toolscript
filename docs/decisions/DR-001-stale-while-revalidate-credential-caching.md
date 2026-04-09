# DR-001: Stale-while-revalidate caching for command-based credentials

## Status
Accepted

## Context
Command-based credential sources (`auth_command`) execute shell commands that may be slow (vault lookups, OAuth token exchanges, cloud provider CLI calls). Per-API-call execution is too expensive, but a simple cache-then-expire model creates latency spikes when credentials expire — every request after expiry blocks until the command completes. The design needed to balance freshness against latency predictability.

## Decision
Three-tier age model with background refresh:
- **Within TTL (default 2s):** cached value returned, no work done.
- **Past TTL, within max age (default 300s):** cached value returned immediately, background refresh spawned. The caller never blocks.
- **Past max age (or no cache):** caller blocks until refresh completes or timeout (default 5s) elapses.

Background refreshes are never aborted — even when the caller falls back to a cached value after timeout, the spawned task runs to completion and updates the cache. On command failure, the cached value is preserved regardless of age.

Rejected alternatives:
- **Per-call execution** — too expensive for commands like `vault read` or `aws sts get-session-token` that take hundreds of milliseconds. Would add latency to every API call.
- **Fixed TTL with blocking refresh** — simple, but creates latency spikes. When the cache expires, the unlucky request that triggers refresh pays the full command execution cost. With short TTLs this happens frequently.
- **Fixed TTL with preemptive refresh (timer-based)** — a background timer refreshes credentials on a schedule regardless of whether they're being used. Wastes resources for idle APIs and adds complexity (timer lifecycle, shutdown coordination). The demand-driven approach only refreshes when credentials are actually needed.
- **Abort-on-timeout** — killing the command on timeout and retrying next time. Rejected because some credential commands are expensive or rate-limited. Letting the in-flight command complete means the next caller benefits from the result even if the triggering caller fell back to cache.

## Consequences
- A slow credential command never blocks the hot path (post-first-call) as long as the cached value is within max age. Latency is predictable.
- First call for any command-based API has no fallback — it must block until the command completes or timeout elapses. If the command exceeds the timeout on first call, the API call proceeds with no credentials (will likely 401).
- The "never abort" behavior means a long-running command consumes a tokio task until completion, even if no one is waiting for it. Acceptable given these are short-lived shell commands, not persistent connections.
- Three configurable knobs (TTL, timeout, max age) with sensible defaults. Most users won't need to tune them, but those with unusual credential commands (very slow, very short-lived tokens) can.
