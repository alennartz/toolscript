import asyncio
import base64
import json
import os
import socket
import subprocess
import time
from contextlib import AsyncExitStack
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
import threading

import httpx
import jwt
import pytest
import pytest_asyncio
from cryptography.hazmat.backends import default_backend
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


# Duplicated from parent conftest.py (module-level functions are not
# inherited across conftest boundaries).
def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _wait_for_http(url: str, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            resp = httpx.get(url, timeout=2.0)
            if resp.status_code < 500:
                return
        except httpx.ConnectError:
            pass
        time.sleep(0.1)
    raise TimeoutError(f"Server at {url} did not start in {timeout}s")


# ---------------------------------------------------------------------------
# JWT issuer fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="session")
def jwt_keys():
    """Generate an RSA key pair for signing and verifying JWTs."""
    private_key = rsa.generate_private_key(
        public_exponent=65537, key_size=2048, backend=default_backend()
    )
    return private_key, private_key.public_key()


@pytest.fixture(scope="session")
def jwks_server(jwt_keys):
    """Run a minimal HTTP server that serves a JWKS document."""
    _, public_key = jwt_keys
    pub_numbers = public_key.public_numbers()

    def _int_to_b64(n, length):
        return base64.urlsafe_b64encode(n.to_bytes(length, "big")).rstrip(b"=").decode()

    jwks_json = json.dumps({"keys": [{
        "kty": "RSA", "use": "sig", "kid": "test-key-1", "alg": "RS256",
        "n": _int_to_b64(pub_numbers.n, 256),
        "e": _int_to_b64(pub_numbers.e, 3),
    }]})

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(jwks_json.encode())

        def log_message(self, *a):
            pass

    port = _free_port()
    server = HTTPServer(("127.0.0.1", port), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{port}"
    server.shutdown()


@pytest.fixture(scope="session")
def sign_jwt(jwt_keys):
    """Return a callable that signs JWTs with the test RSA key."""
    private_key, _ = jwt_keys
    pem = private_key.private_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PrivateFormat.PKCS8,
        encryption_algorithm=serialization.NoEncryption(),
    )

    def _sign(audience="test-audience", issuer="test-issuer", exp_seconds=3600):
        now = int(time.time())
        return jwt.encode(
            {"sub": "test-user", "aud": audience, "iss": issuer, "iat": now, "exp": now + exp_seconds},
            pem, algorithm="RS256", headers={"kid": "test-key-1"},
        )
    return _sign


# ---------------------------------------------------------------------------
# HTTP transport fixtures
# ---------------------------------------------------------------------------


@pytest_asyncio.fixture(loop_scope="session", scope="session")
async def mcp_http_session(code_mcp_binary, openapi_spec_url, jwks_server, sign_jwt):
    """Spawn code-mcp with HTTP transport + JWT auth, connect an MCP client.

    If CODE_MCP_URL is set, connect to the external server instead.
    """
    from mcp.client.streamable_http import streamable_http_client

    external_url = os.environ.get("CODE_MCP_URL")
    if external_url:
        base_url = external_url
        proc = None
    else:
        port = _free_port()
        env = {
            "PATH": "/usr/bin:/bin",
            "TEST_API_BEARER_TOKEN": "test-secret-123",
        }
        proc = subprocess.Popen(
            [
                str(code_mcp_binary), "run", openapi_spec_url,
                "--auth", "TEST_API_BEARER_TOKEN",
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
    if proc is not None:
        proc.terminate()
        proc.wait(timeout=5)


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
            "--auth", "TEST_API_BEARER_TOKEN",
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


@pytest_asyncio.fixture(loop_scope="session", scope="session")
async def mcp_stdio_session(code_mcp_binary: Path, openapi_spec_url: str):
    """Spawn code-mcp and connect an MCP client over stdio.

    Uses a background task to manage the stdio_client + ClientSession
    context managers so that setup and teardown both run in the same
    task, avoiding anyio cancel-scope cross-task errors.
    """
    env = {
        "PATH": "/usr/bin:/bin",
        "TEST_API_BEARER_TOKEN": "test-secret-123",
        "TEST_API_API_KEY": "test-key-456",
    }
    server_params = StdioServerParameters(
        command=str(code_mcp_binary),
        args=["run", openapi_spec_url, "--auth", "TEST_API_BEARER_TOKEN"],
        env=env,
    )

    session_ready: asyncio.Future[ClientSession] = asyncio.get_event_loop().create_future()
    shutdown_event = asyncio.Event()

    async def _run_session():
        try:
            async with stdio_client(server_params) as (read, write):
                async with ClientSession(read, write) as session:
                    await session.initialize()
                    session_ready.set_result(session)
                    # Block until tests signal shutdown
                    await shutdown_event.wait()
        except Exception as exc:
            if not session_ready.done():
                session_ready.set_exception(exc)

    task = asyncio.create_task(_run_session())
    session = await session_ready
    yield session
    shutdown_event.set()
    # Give the task a moment to clean up gracefully
    try:
        await asyncio.wait_for(task, timeout=5.0)
    except (asyncio.TimeoutError, Exception):
        task.cancel()
        try:
            await task
        except (asyncio.CancelledError, Exception):
            pass


@pytest_asyncio.fixture(loop_scope="session")
async def mcp_no_auth_session(code_mcp_binary: Path, openapi_spec_url: str):
    """code-mcp instance with NO upstream API credentials."""
    env = {"PATH": "/usr/bin:/bin"}
    server_params = StdioServerParameters(
        command=str(code_mcp_binary),
        args=["run", openapi_spec_url],
        env=env,
    )
    session_ready = asyncio.get_event_loop().create_future()
    shutdown_event = asyncio.Event()

    async def _run():
        try:
            async with stdio_client(server_params) as (read, write):
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


@pytest_asyncio.fixture(loop_scope="session", scope="session")
async def mcp_limited_session(code_mcp_binary: Path, openapi_spec_url: str):
    """code-mcp instance with short execution limits."""
    env = {
        "PATH": "/usr/bin:/bin",
        "TEST_API_BEARER_TOKEN": "test-secret-123",
    }
    server_params = StdioServerParameters(
        command=str(code_mcp_binary),
        args=[
            "run", openapi_spec_url,
            "--auth", "TEST_API_BEARER_TOKEN",
            "--timeout", "2",
            "--max-api-calls", "3",
        ],
        env=env,
    )
    session_ready = asyncio.get_event_loop().create_future()
    shutdown_event = asyncio.Event()

    async def _run():
        try:
            async with stdio_client(server_params) as (read, write):
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
