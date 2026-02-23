import pytest
from mcp import ClientSession


@pytest.mark.asyncio
async def test_list_tools(mcp_stdio_session: ClientSession):
    """Verify that the MCP server exposes the expected tools."""
    result = await mcp_stdio_session.list_tools()
    tool_names = {t.name for t in result.tools}
    assert "list_apis" in tool_names
    assert "list_functions" in tool_names
    assert "get_function_docs" in tool_names
    assert "search_docs" in tool_names
    assert "get_schema" in tool_names
    assert "execute_script" in tool_names


@pytest.mark.asyncio
async def test_list_apis(mcp_stdio_session: ClientSession):
    result = await mcp_stdio_session.call_tool("list_apis", {})
    text = result.content[0].text
    assert "test_api" in text


@pytest.mark.asyncio
async def test_list_functions(mcp_stdio_session: ClientSession):
    result = await mcp_stdio_session.call_tool("list_functions", {})
    text = result.content[0].text
    assert "list_pets" in text
    assert "create_pet" in text
    assert "get_pet" in text


@pytest.mark.asyncio
async def test_list_functions_filter_by_tag(mcp_stdio_session: ClientSession):
    result = await mcp_stdio_session.call_tool("list_functions", {"tag": "pets"})
    text = result.content[0].text
    assert "list_pets" in text


@pytest.mark.asyncio
async def test_get_function_docs(mcp_stdio_session: ClientSession):
    result = await mcp_stdio_session.call_tool("get_function_docs", {"name": "list_pets"})
    text = result.content[0].text
    assert "list_pets" in text


@pytest.mark.asyncio
async def test_search_docs(mcp_stdio_session: ClientSession):
    result = await mcp_stdio_session.call_tool("search_docs", {"query": "pet"})
    text = result.content[0].text
    assert "pet" in text.lower()


@pytest.mark.asyncio
async def test_get_schema(mcp_stdio_session: ClientSession):
    result = await mcp_stdio_session.call_tool("get_schema", {"name": "Pet"})
    text = result.content[0].text
    assert "name" in text.lower()
