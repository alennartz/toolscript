# GitHub Actions CI/CD Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a CI/CD pipeline that runs checks + e2e tests on PRs, and deploys a minimal Docker image to GHCR on merge to main.

**Architecture:** Single workflow with 4 jobs: check -> (e2e-binary, e2e-docker) -> deploy. The Dockerfile is improved to produce a static musl binary in a FROM scratch image. E2e test fixtures gain a CODE_MCP_URL env var so HTTP tests can target an external Docker container.

**Tech Stack:** GitHub Actions, Swatinem/rust-cache, docker/build-push-action, docker/metadata-action, docker/login-action, dtolnay/rust-toolchain, actions/setup-python

---

### Task 1: Switch reqwest to rustls-tls

**Files:**
- Modify: `Cargo.toml:14`

**Step 1: Update Cargo.toml**

Change the reqwest dependency from:
```toml
reqwest = { version = "0.12", features = ["json"] }
```
to:
```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

This drops the native-tls (OpenSSL) dependency and uses pure-Rust rustls instead, enabling fully static musl builds.

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Builds without errors.

**Step 3: Run tests to confirm nothing breaks**

Run: `cargo test`
Expected: All tests pass. The TLS backend change is transparent to the code.

**Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: switch reqwest from native-tls to rustls-tls"
```

---

### Task 2: Rewrite Dockerfile for static musl build

**Files:**
- Modify: `Dockerfile`

**Step 1: Write the new Dockerfile**

Replace the entire Dockerfile with:

```dockerfile
FROM rust:1.85-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y \
        musl-tools musl-dev g++ pkg-config \
    && rustup target add x86_64-unknown-linux-musl \
    && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM scratch
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/code-mcp /code-mcp
ENTRYPOINT ["/code-mcp", "run"]
```

Key points:
- `musl-tools musl-dev` for the musl C toolchain
- `g++` for compiling mlua's Luau C++ code
- `pkg-config` kept because some crates use it during build
- Builder copies CA certs to the scratch image for TLS
- Binary path changes to the musl target directory

**Step 2: Build the Docker image locally**

Run: `docker build -t code-mcp:test .`
Expected: Builds successfully. Watch for any linker errors related to musl or C++ compilation.

**Step 3: Verify the image runs**

Run: `docker run --rm code-mcp:test --help`
Expected: Prints the code-mcp help text. This confirms the static binary runs in the scratch container.

**Step 4: Check image size**

Run: `docker images code-mcp:test`
Expected: Image size should be ~15-30 MB (just binary + CA certs), versus ~80MB+ for the old bookworm-slim image.

**Step 5: Commit**

```bash
git add Dockerfile
git commit -m "build: static musl binary with FROM scratch image"
```

---

### Task 3: Add CODE_MCP_URL support to e2e test fixtures

**Files:**
- Modify: `e2e/tests/conftest.py:115-201` (the `mcp_http_session` and `mcp_http_url` fixtures)

The HTTP test fixtures currently always launch a local binary process. We need them to optionally connect to an externally-running server when `CODE_MCP_URL` is set.

**Step 1: Write a test to verify the CODE_MCP_URL path works**

Create file `e2e/tests/test_docker_mode.py`:

```python
"""Verify that CODE_MCP_URL mode correctly skips binary launch."""
import os
import pytest


def test_code_mcp_url_env_skips_binary(monkeypatch):
    """When CODE_MCP_URL is set, fixtures should use it instead of launching a binary."""
    # This is a unit test for the fixture logic, not an integration test.
    # It verifies the env var is read correctly.
    monkeypatch.setenv("CODE_MCP_URL", "http://localhost:9999")
    assert os.environ.get("CODE_MCP_URL") == "http://localhost:9999"
```

Run: `cd e2e && python -m pytest tests/test_docker_mode.py -v`
Expected: PASS

**Step 2: Modify the `mcp_http_url` fixture**

In `e2e/tests/conftest.py`, change the `mcp_http_url` fixture (line 177) to check for `CODE_MCP_URL` first:

```python
@pytest.fixture(scope="session")
def mcp_http_url(code_mcp_binary, openapi_spec_url, jwks_server):
    """Spawn code-mcp with HTTP transport + JWT auth, yield the base URL.

    If CODE_MCP_URL is set, skip spawning and use the external server.
    """
    external_url = os.environ.get("CODE_MCP_URL")
    if external_url:
        yield external_url
        return

    port = _free_port()
    env = {
        "PATH": "/usr/bin:/bin",
        "TEST_API_BEARER_TOKEN": "test-secret-123",
    }
    proc = subprocess.Popen(
        [
            str(code_mcp_binary), "run", openapi_spec_url,
            "--transport", "http", "--port", str(port),
            "--auth-authority", "test-issuer",
            "--auth-audience", "test-audience",
            "--auth-jwks-uri", f"{jwks_server}/jwks",
        ],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    url = f"http://127.0.0.1:{port}"
    _wait_for_http(f"{url}/.well-known/oauth-protected-resource")
    yield url
    proc.terminate()
    proc.wait(timeout=5)
```

**Step 3: Modify the `mcp_http_session` fixture**

Same pattern for `mcp_http_session` (line 115). When `CODE_MCP_URL` is set, connect to the external URL instead of spawning a process:

```python
@pytest_asyncio.fixture(loop_scope="session", scope="session")
async def mcp_http_session(code_mcp_binary, openapi_spec_url, jwks_server, sign_jwt):
    """Spawn code-mcp with HTTP transport + JWT auth, connect an MCP client.

    If CODE_MCP_URL is set, connect to the external server instead.
    """
    from mcp.client.streamable_http import streamable_http_client

    external_url = os.environ.get("CODE_MCP_URL")
    if external_url:
        base_url = external_url
    else:
        port = _free_port()
        env = {
            "PATH": "/usr/bin:/bin",
            "TEST_API_BEARER_TOKEN": "test-secret-123",
        }
        proc = subprocess.Popen(
            [
                str(code_mcp_binary), "run", openapi_spec_url,
                "--transport", "http", "--port", str(port),
                "--auth-authority", "test-issuer",
                "--auth-audience", "test-audience",
                "--auth-jwks-uri", f"{jwks_server}/jwks",
            ],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        base_url = f"http://127.0.0.1:{port}"
        _wait_for_http(f"{base_url}/.well-known/oauth-protected-resource")

    token = sign_jwt()
    headers = {"Authorization": f"Bearer {token}"}

    session_ready = asyncio.get_event_loop().create_future()
    shutdown_event = asyncio.Event()

    async def _run():
        try:
            async with streamable_http_client(
                f"{base_url}/mcp",
                http_client=httpx.AsyncClient(headers=headers),
            ) as (read, write, _):
                async with ClientSession(read, write) as session:
                    await session.initialize()
                    session_ready.set_result(session)
                    await shutdown_event.wait()
        except Exception as exc:
            if not session_ready.done():
                session_ready.set_exception(exc)

    task = asyncio.create_task(_run())
    session = await session_ready
    yield session
    shutdown_event.set()
    try:
        await asyncio.wait_for(task, timeout=5.0)
    except (asyncio.TimeoutError, Exception):
        task.cancel()
        try:
            await task
        except (asyncio.CancelledError, Exception):
            pass
    if not external_url:
        proc.terminate()
        proc.wait(timeout=5)
```

Note: `os` import is already needed — add `import os` at the top of `e2e/tests/conftest.py` if not already present.

**Step 4: Run the full e2e suite to confirm no regression**

Run: `cd e2e && python -m pytest tests/ -v`
Expected: All tests pass. Without `CODE_MCP_URL` set, behavior is identical to before.

**Step 5: Clean up the test_docker_mode.py file**

Delete `e2e/tests/test_docker_mode.py` — it was a sanity check, not a permanent test.

**Step 6: Commit**

```bash
git add e2e/tests/conftest.py
git commit -m "test: add CODE_MCP_URL support for running HTTP e2e tests against external server"
```

---

### Task 4: Create the GitHub Actions workflow

**Files:**
- Create: `.github/workflows/ci.yml`

**Step 1: Create the workflow directory**

```bash
mkdir -p .github/workflows
```

**Step 2: Write the workflow file**

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

permissions:
  contents: read
  packages: write

jobs:
  check:
    name: Check
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy

      - uses: Swatinem/rust-cache@v2

      - name: Format
        run: cargo fmt --check

      - name: Clippy
        run: cargo clippy -- -D warnings

      - name: Test
        run: cargo test

  e2e-binary:
    name: E2E (binary)
    needs: check
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - name: Build release binary
        run: cargo build --release

      - uses: actions/setup-python@v5
        with:
          python-version: "3.11"

      - name: Install e2e dependencies
        run: pip install -e .
        working-directory: e2e

      - name: Run e2e tests
        run: python -m pytest tests/ -v
        working-directory: e2e

  e2e-docker:
    name: E2E (docker)
    needs: check
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build Docker image
        uses: docker/build-push-action@v6
        with:
          context: .
          push: false
          load: true
          tags: code-mcp:ci

      - uses: actions/setup-python@v5
        with:
          python-version: "3.11"

      - name: Install e2e dependencies
        run: pip install -e .
        working-directory: e2e

      - name: Start test API server
        run: |
          cd e2e && python -m uvicorn test_api.app:app \
            --host 127.0.0.1 --port 9100 --log-level warning &
          sleep 2
          curl -sf http://127.0.0.1:9100/openapi.json > /dev/null
        env:
          TEST_API_SERVER_URL: "http://127.0.0.1:9100"

      - name: Start JWKS server and code-mcp container
        run: |
          cd e2e && python -c "
          import json, base64, time, threading, socket
          from http.server import BaseHTTPRequestHandler, HTTPServer
          from cryptography.hazmat.backends import default_backend
          from cryptography.hazmat.primitives.asymmetric import rsa
          from cryptography.hazmat.primitives import serialization

          private_key = rsa.generate_private_key(65537, 2048, default_backend())
          public_key = private_key.public_key()
          pub_numbers = public_key.public_numbers()

          def _int_to_b64(n, length):
              return base64.urlsafe_b64encode(n.to_bytes(length, 'big')).rstrip(b'=').decode()

          jwks_json = json.dumps({'keys': [{
              'kty': 'RSA', 'use': 'sig', 'kid': 'test-key-1', 'alg': 'RS256',
              'n': _int_to_b64(pub_numbers.n, 256),
              'e': _int_to_b64(pub_numbers.e, 3),
          }]})

          # Save private key for token signing
          pem = private_key.private_bytes(
              encoding=serialization.Encoding.PEM,
              format=serialization.PrivateFormat.PKCS8,
              encryption_algorithm=serialization.NoEncryption(),
          )
          with open('/tmp/jwt_private_key.pem', 'wb') as f:
              f.write(pem)

          class Handler(BaseHTTPRequestHandler):
              def do_GET(self):
                  self.send_response(200)
                  self.send_header('Content-Type', 'application/json')
                  self.end_headers()
                  self.wfile.write(jwks_json.encode())
              def log_message(self, *a): pass

          server = HTTPServer(('127.0.0.1', 9200), Handler)
          t = threading.Thread(target=server.serve_forever, daemon=True)
          t.start()
          print('JWKS server on :9200')
          import time; time.sleep(999999)
          " &
          sleep 1

          # Start the code-mcp container
          docker run -d --name code-mcp-ci --network=host \
            -e TEST_API_BEARER_TOKEN=test-secret-123 \
            code-mcp:ci \
            http://127.0.0.1:9100/openapi.json \
            --transport http --port 9300 \
            --auth-authority test-issuer \
            --auth-audience test-audience \
            --auth-jwks-uri http://127.0.0.1:9200/jwks

          # Wait for code-mcp to be ready
          for i in $(seq 1 30); do
            if curl -sf http://127.0.0.1:9300/.well-known/oauth-protected-resource > /dev/null 2>&1; then
              echo "code-mcp ready"
              break
            fi
            sleep 1
          done

      - name: Run HTTP e2e tests against Docker container
        run: |
          python -m pytest tests/test_http_transport.py tests/test_auth.py -v \
            --override-ini="asyncio_default_fixture_loop_scope=session"
        working-directory: e2e
        env:
          CODE_MCP_URL: "http://127.0.0.1:9300"

      - name: Dump container logs on failure
        if: failure()
        run: docker logs code-mcp-ci

  deploy:
    name: Deploy to GHCR
    needs: [e2e-binary, e2e-docker]
    if: github.event_name == 'push' && github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Docker metadata
        id: meta
        uses: docker/metadata-action@v5
        with:
          images: ghcr.io/${{ github.repository }}
          tags: |
            type=ref,event=branch
            type=sha,prefix=sha-

      - name: Build and push
        uses: docker/build-push-action@v6
        with:
          context: .
          push: true
          tags: ${{ steps.meta.outputs.tags }}
          labels: ${{ steps.meta.outputs.labels }}
```

**Step 3: Validate the YAML syntax**

Run: `python -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`
Expected: No errors. (Requires pyyaml: `pip install pyyaml` if not present.)

**Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add GitHub Actions workflow with check, e2e, and GHCR deploy"
```

---

### Task 5: Integration smoke test

This task verifies the full chain works locally before pushing.

**Step 1: Run cargo test**

Run: `cargo test`
Expected: All Rust tests pass with the rustls-tls switch.

**Step 2: Build the Docker image**

Run: `docker build -t code-mcp:test .`
Expected: Builds successfully with musl target.

**Step 3: Run e2e tests against binary**

Run: `cd e2e && python -m pytest tests/ -v`
Expected: All tests pass.

**Step 4: Run e2e HTTP tests against Docker container**

This replicates what the e2e-docker CI job does:

```bash
# Start test API
cd e2e && TEST_API_SERVER_URL=http://127.0.0.1:9100 \
  python -m uvicorn test_api.app:app --host 127.0.0.1 --port 9100 --log-level warning &

# Start code-mcp container (no auth for quick smoke test)
docker run -d --name smoke --network=host \
  -e TEST_API_BEARER_TOKEN=test-secret-123 \
  code-mcp:test \
  http://127.0.0.1:9100/openapi.json \
  --transport http --port 9300

# Wait for it
sleep 3

# Run HTTP tests
cd e2e && CODE_MCP_URL=http://127.0.0.1:9300 \
  python -m pytest tests/test_http_transport.py -v -k "not auth_required and not well_known"

# Clean up
docker rm -f smoke
kill %1
```

Note: The full auth tests need the JWKS server. For a quick smoke, run just the non-auth HTTP tests.

**Step 5: Commit all changes together if any fixups were needed**

```bash
git add -A
git commit -m "ci: fixups from integration smoke test"
```
