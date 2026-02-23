import asyncio
from contextlib import AsyncExitStack
from pathlib import Path

import pytest_asyncio
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


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
        args=["run", openapi_spec_url],
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
