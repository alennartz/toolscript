"""Mixed mode tests: OpenAPI + MCP together.

Verify that both OpenAPI-generated functions and MCP tools coexist
correctly when toolscript is run with both a spec and --mcp flags.
"""

import json

import pytest
from mcp import ClientSession

from helpers import unwrap as _unwrap


@pytest.mark.asyncio
async def test_list_apis_shows_both_sources(mcp_mixed_session: ClientSession):
    """list_apis returns both the OpenAPI API and MCP server."""
    result = await mcp_mixed_session.call_tool("list_apis", {})
    apis = json.loads(result.content[0].text)
    names = {a["name"] for a in apis}
    assert "test_api" in names, f"Missing OpenAPI API. Got: {names}"
    assert "mock" in names, f"Missing MCP server. Got: {names}"
    mock_api = next(a for a in apis if a["name"] == "mock")
    assert mock_api["source"] == "mcp"


@pytest.mark.asyncio
async def test_list_functions_includes_both(mcp_mixed_session: ClientSession):
    """list_functions returns both OpenAPI functions and MCP tools."""
    result = await mcp_mixed_session.call_tool("list_functions", {})
    functions = json.loads(result.content[0].text)
    names = {f["name"] for f in functions}
    assert "list_pets" in names, f"Missing OpenAPI function. Got: {names}"
    assert "echo" in names, f"Missing MCP tool. Got: {names}"
    assert "add" in names, f"Missing MCP tool. Got: {names}"


@pytest.mark.asyncio
async def test_openapi_call_still_works(mcp_mixed_session: ClientSession):
    """sdk.list_pets() executes successfully in mixed mode."""
    result = await mcp_mixed_session.call_tool(
        "execute_script", {"script": "return sdk.list_pets()"}
    )
    text = result.content[0].text
    parsed = json.loads(text)
    assert "error" not in parsed or parsed.get("error") is None


@pytest.mark.asyncio
async def test_mcp_call_still_works(mcp_mixed_session: ClientSession):
    """sdk.mock.echo works in mixed mode."""
    result = await mcp_mixed_session.call_tool(
        "execute_script",
        {"script": 'return sdk.mock.echo({ text = "hi" })'},
    )
    parsed = json.loads(result.content[0].text)
    assert _unwrap(parsed) == "hi"


@pytest.mark.asyncio
async def test_mixed_script(mcp_mixed_session: ClientSession):
    """Single script calls both OpenAPI and MCP tools."""
    result = await mcp_mixed_session.call_tool(
        "execute_script",
        {
            "script": """
local pets = sdk.list_pets()
local echo_raw = sdk.mock.echo({ text = "mixed" })
local echoed = type(echo_raw) == "table" and echo_raw.result or echo_raw
return tostring(pets) .. "|" .. tostring(echoed)
"""
        },
    )
    text = result.content[0].text
    parsed = json.loads(text)
    result_str = _unwrap(parsed)
    assert "mixed" in result_str


@pytest.mark.asyncio
async def test_search_docs_spans_both(mcp_mixed_session: ClientSession):
    """search_docs for 'list' returns matches from both OpenAPI and MCP."""
    result = await mcp_mixed_session.call_tool(
        "search_docs", {"query": "list"}
    )
    results = json.loads(result.content[0].text)
    types = {r.get("type") for r in results}
    assert "function" in types, f"No OpenAPI match. Types: {types}"
    assert "mcp_tool" in types, f"No MCP match. Types: {types}"


@pytest.mark.asyncio
async def test_get_function_docs_openapi_unchanged(mcp_mixed_session: ClientSession):
    """get_function_docs for OpenAPI function works normally in mixed mode."""
    result = await mcp_mixed_session.call_tool(
        "get_function_docs", {"name": "list_pets"}
    )
    text = result.content[0].text
    assert "list_pets" in text
    assert "function sdk" in text
