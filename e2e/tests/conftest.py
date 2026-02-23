from pathlib import Path

import pytest_asyncio
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


@pytest_asyncio.fixture(scope="session")
async def mcp_stdio_session(code_mcp_binary: Path, openapi_spec_url: str):
    """Spawn code-mcp and connect an MCP client over stdio."""
    env = {
        "PATH": "/usr/bin:/bin",
        "TEST_API_BEARER_TOKEN": "test-secret-123",
        "TEST_API_API_KEY": "test-key-456",
    }
    server_params = StdioServerParameters(
        command=str(code_mcp_binary),
        args=["run", openapi_spec_url],
        env=env,
    )
    async with stdio_client(server_params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            yield session
