# GitHub Actions CI/CD Design

## Overview

Add a CI/CD pipeline via GitHub Actions that runs tests on PRs and pushes, and deploys a Docker image to GHCR on merge to main. Also improve the Dockerfile to produce a minimal static binary image.

## Pipeline Structure

Single workflow file: `.github/workflows/ci.yml`

```
check ──→ e2e-binary ──→ deploy
     └──→ e2e-docker ──┘
```

### Triggers

- `pull_request` to `main` — runs check + both e2e jobs
- `push` to `main` — runs check + both e2e jobs + deploy

### Jobs

#### `check`

Runs on `ubuntu-latest` with Rust dependency caching (`Swatinem/rust-cache`).

Steps:
1. Checkout
2. Install Rust stable (`dtolnay/rust-toolchain`)
3. Rust cache
4. `cargo fmt --check`
5. `cargo clippy -- -D warnings`
6. `cargo test`

#### `e2e-binary`

Needs `check`. Runs the full e2e test suite against a natively-built release binary.

Steps:
1. Checkout
2. Install Rust stable + rust-cache
3. `cargo build --release`
4. Setup Python 3.11 (`actions/setup-python`)
5. `pip install -e .` in `e2e/`
6. `pytest` in `e2e/` (full suite — stdio + HTTP tests)

#### `e2e-docker`

Needs `check`. Builds the Docker image and runs HTTP/SSE transport tests against a running container.

Steps:
1. Checkout
2. Build Docker image locally (`docker/build-push-action` with `push: false`, `load: true`)
3. Setup Python 3.11
4. `pip install -e .` in `e2e/`
5. Start test API server (Python/uvicorn) on host
6. Run Docker container with `--network=host`, pointing at the test API's OpenAPI spec
7. Run HTTP-only e2e tests (`pytest tests/test_http_transport.py tests/test_auth.py`) with `CODE_MCP_URL` env var pointing at the container's SSE endpoint

#### `deploy`

Needs `e2e-binary` + `e2e-docker`. Only runs on `push` to `main`.

Steps:
1. Checkout
2. Log in to GHCR (`docker/login-action` with `GITHUB_TOKEN`)
3. Extract metadata — tags: `main`, `sha-<short>` (`docker/metadata-action`)
4. Build and push (`docker/build-push-action` with `push: true`)

Workflow permissions: `packages: write`, `contents: read`.

## Dockerfile Improvement

Switch from `debian:bookworm-slim` runtime to `FROM scratch` with a fully static musl binary.

### Cargo.toml change

```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

Switches from native-tls (OpenSSL, dynamic linking) to rustls (pure Rust, static).

### New Dockerfile

- **Builder:** `rust:1.85-slim`, install `musl-tools musl-dev g++` (g++ needed for mlua's Luau C++ compilation), add `x86_64-unknown-linux-musl` target, build with `--target x86_64-unknown-linux-musl --release`
- **Production:** `FROM scratch`, copy CA certificates from builder, copy static binary, set entrypoint

Reduces image from ~80MB to ~15-20MB.

## E2E Test Fixture Change

Add `CODE_MCP_URL` env var support to the HTTP test fixtures. When set, HTTP/SSE tests connect to the provided URL instead of launching a binary process. This allows tests to target an externally-running Docker container.

Affected file: `e2e/tests/conftest.py` or `e2e/conftest.py` (wherever the HTTP fixtures are defined).
