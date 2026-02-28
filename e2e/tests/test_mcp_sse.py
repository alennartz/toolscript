"""SSE transport e2e tests.

Verify that connecting to a legacy SSE MCP server via URL works
without any special transport config.
"""

import json

import pytest
from mcp import ClientSession

from helpers import exec_script as _exec, unwrap as _unwrap


@pytest.mark.asyncio
async def test_sse_echo(mcp_sse_session: ClientSession):
    """sdk.sse_mock.echo returns the input text over SSE transport."""
    parsed = await _exec(
        mcp_sse_session,
        'return sdk.sse_mock.echo({ text = "hello-sse" })',
    )
    assert _unwrap(parsed) == "hello-sse"


@pytest.mark.asyncio
async def test_sse_add(mcp_sse_session: ClientSession):
    """sdk.sse_mock.add returns the sum over SSE transport."""
    parsed = await _exec(
        mcp_sse_session,
        "return sdk.sse_mock.add({ a = 10, b = 20 })",
    )
    assert _unwrap(parsed) == 30


@pytest.mark.asyncio
async def test_sse_list_functions(mcp_sse_session: ClientSession):
    """list_functions includes SSE server tools."""
    result = await mcp_sse_session.call_tool("list_functions", {})
    functions = json.loads(result.content[0].text)
    names = {f["name"] for f in functions}
    assert "echo" in names, f"Missing echo tool. Got: {names}"
    assert "add" in names, f"Missing add tool. Got: {names}"
