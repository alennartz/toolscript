"""Discovery tests for MCP-only mode.

Verify that MCP tools are properly discoverable through the standard
toolscript MCP server interface (list_apis, list_functions, get_function_docs,
search_docs).
"""

import json

import pytest
from mcp import ClientSession


@pytest.mark.asyncio
async def test_list_tools_in_mcp_only_mode(mcp_only_session: ClientSession):
    """Standard toolscript tools are still exposed in MCP-only mode."""
    result = await mcp_only_session.list_tools()
    tool_names = {t.name for t in result.tools}
    assert "list_apis" in tool_names
    assert "list_functions" in tool_names
    assert "get_function_docs" in tool_names
    assert "search_docs" in tool_names
    assert "execute_script" in tool_names


@pytest.mark.asyncio
async def test_list_apis_shows_mcp_server(mcp_only_session: ClientSession):
    """list_apis returns the mock MCP server with source: 'mcp'."""
    result = await mcp_only_session.call_tool("list_apis", {})
    apis = json.loads(result.content[0].text)
    mock_api = next((a for a in apis if a["name"] == "mock"), None)
    assert mock_api is not None, f"No 'mock' API found in: {apis}"
    assert mock_api["source"] == "mcp"
    assert mock_api["tool_count"] == 6


@pytest.mark.asyncio
async def test_list_functions_shows_all_mcp_tools(mcp_only_session: ClientSession):
    """list_functions returns all 6 mock MCP tools."""
    result = await mcp_only_session.call_tool("list_functions", {})
    functions = json.loads(result.content[0].text)
    names = {f["name"] for f in functions}
    expected = {"echo", "add", "get_user", "list_items", "failing_tool", "no_params"}
    assert expected.issubset(names), f"Missing tools. Got: {names}"


@pytest.mark.asyncio
async def test_list_functions_mcp_source_field(mcp_only_session: ClientSession):
    """Each MCP function has source: 'mcp' and api: 'mock'."""
    result = await mcp_only_session.call_tool("list_functions", {})
    functions = json.loads(result.content[0].text)
    for fn in functions:
        assert fn["source"] == "mcp", f"{fn['name']} missing source=mcp"
        assert fn["api"] == "mock", f"{fn['name']} missing api=mock"


@pytest.mark.asyncio
async def test_list_functions_filter_by_api(mcp_only_session: ClientSession):
    """list_functions with api='mock' returns only mock tools."""
    result = await mcp_only_session.call_tool("list_functions", {"api": "mock"})
    functions = json.loads(result.content[0].text)
    assert len(functions) == 6
    assert all(f["api"] == "mock" for f in functions)


@pytest.mark.asyncio
async def test_get_function_docs_echo(mcp_only_session: ClientSession):
    """get_function_docs for mock.echo returns a Luau annotation with text param."""
    result = await mcp_only_session.call_tool(
        "get_function_docs", {"name": "mock.echo"}
    )
    text = result.content[0].text
    assert "function sdk.mock.echo" in text
    assert "text: string" in text


@pytest.mark.asyncio
async def test_get_function_docs_typed_params(mcp_only_session: ClientSession):
    """mock.add docs show numeric params."""
    result = await mcp_only_session.call_tool(
        "get_function_docs", {"name": "mock.add"}
    )
    text = result.content[0].text
    assert "a: number" in text
    assert "b: number" in text


@pytest.mark.asyncio
async def test_get_function_docs_optional_params(mcp_only_session: ClientSession):
    """mock.get_user docs show include_email as optional."""
    result = await mcp_only_session.call_tool(
        "get_function_docs", {"name": "mock.get_user"}
    )
    text = result.content[0].text
    assert "user_id: string" in text
    assert "include_email" in text


@pytest.mark.asyncio
async def test_get_function_docs_no_params(mcp_only_session: ClientSession):
    """mock.no_params docs show empty param list."""
    result = await mcp_only_session.call_tool(
        "get_function_docs", {"name": "mock.no_params"}
    )
    text = result.content[0].text
    assert "function sdk.mock.no_params" in text


@pytest.mark.asyncio
async def test_search_docs_finds_mcp_tool(mcp_only_session: ClientSession):
    """search_docs for 'echo' finds the mock.echo MCP tool."""
    result = await mcp_only_session.call_tool("search_docs", {"query": "echo"})
    results = json.loads(result.content[0].text)
    mcp_hits = [r for r in results if r.get("type") == "mcp_tool"]
    assert any("echo" in r["name"] for r in mcp_hits), f"No echo hit in: {results}"


@pytest.mark.asyncio
async def test_search_docs_by_description(mcp_only_session: ClientSession):
    """search_docs for 'Add two numbers' finds mock.add."""
    result = await mcp_only_session.call_tool(
        "search_docs", {"query": "Add two numbers"}
    )
    results = json.loads(result.content[0].text)
    names = [r["name"] for r in results]
    assert any("add" in n for n in names), f"No add hit in: {names}"
