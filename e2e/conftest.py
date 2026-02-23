import socket
import subprocess
import threading
import time
from pathlib import Path

import httpx
import pytest
import uvicorn

PROJECT_ROOT = Path(__file__).resolve().parent.parent
CODE_MCP_BINARY = PROJECT_ROOT / "target" / "release" / "code-mcp"


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


@pytest.fixture(scope="session")
def code_mcp_binary() -> Path:
    if not CODE_MCP_BINARY.exists():
        subprocess.run(
            ["cargo", "build", "--release"],
            cwd=PROJECT_ROOT,
            check=True,
        )
    return CODE_MCP_BINARY


@pytest.fixture(scope="session")
def test_api_url() -> str:
    port = _free_port()
    config = uvicorn.Config(
        "test_api.app:app",
        host="127.0.0.1",
        port=port,
        log_level="warning",
    )
    server = uvicorn.Server(config)
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{port}"
    _wait_for_http(f"{url}/openapi.json")
    yield url
    server.should_exit = True
    thread.join(timeout=5)


@pytest.fixture(scope="session")
def openapi_spec_url(test_api_url: str) -> str:
    return f"{test_api_url}/openapi.json"


@pytest.fixture(autouse=True)
def reset_test_data(test_api_url: str) -> None:
    httpx.post(f"{test_api_url}/reset", timeout=5.0)
