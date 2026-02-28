"""Script execution tests for MCP tools.

Verify that Luau scripts can call MCP tools via sdk.mock.<tool>() and
handle various parameter types, return values, and error conditions.
"""

import json

import pytest
from mcp import ClientSession

from helpers import exec_script as _exec, unwrap as _unwrap


@pytest.mark.asyncio
async def test_echo_tool(mcp_only_session: ClientSession):
    """sdk.mock.echo returns the input text."""
    parsed = await _exec(
        mcp_only_session,
        'return sdk.mock.echo({ text = "hello" })',
    )
    assert _unwrap(parsed) == "hello"


@pytest.mark.asyncio
async def test_add_tool(mcp_only_session: ClientSession):
    """sdk.mock.add returns the sum."""
    parsed = await _exec(
        mcp_only_session,
        "return sdk.mock.add({ a = 2, b = 3 })",
    )
    assert _unwrap(parsed) == 5


@pytest.mark.asyncio
async def test_get_user_basic(mcp_only_session: ClientSession):
    """sdk.mock.get_user returns user data with name."""
    parsed = await _exec(
        mcp_only_session,
        'return sdk.mock.get_user({ user_id = "u1" })',
    )
    result_str = json.dumps(_unwrap(parsed))
    assert "Alice" in result_str


@pytest.mark.asyncio
async def test_get_user_optional_param(mcp_only_session: ClientSession):
    """sdk.mock.get_user with include_email=true includes email."""
    parsed = await _exec(
        mcp_only_session,
        'return sdk.mock.get_user({ user_id = "u1", include_email = true })',
    )
    result_str = json.dumps(_unwrap(parsed))
    assert "email" in result_str


@pytest.mark.asyncio
async def test_list_items(mcp_only_session: ClientSession):
    """sdk.mock.list_items returns items."""
    parsed = await _exec(
        mcp_only_session,
        'return sdk.mock.list_items({ category = "books" })',
    )
    items = _unwrap(parsed)
    assert isinstance(items, list)
    assert len(items) == 3
    assert items[0]["name"] == "books-0"


@pytest.mark.asyncio
async def test_no_params_tool(mcp_only_session: ClientSession):
    """sdk.mock.no_params() works with no arguments."""
    parsed = await _exec(
        mcp_only_session,
        "return sdk.mock.no_params()",
    )
    assert _unwrap(parsed) == "ok"


@pytest.mark.asyncio
async def test_failing_tool_error(mcp_only_session: ClientSession):
    """pcall on sdk.mock.failing_tool captures the error."""
    parsed = await _exec(
        mcp_only_session,
        """
local ok, err = pcall(sdk.mock.failing_tool)
if not ok then
    return "caught: " .. tostring(err)
end
return "unexpected success"
""",
    )
    result = _unwrap(parsed)
    assert "caught" in result


@pytest.mark.asyncio
async def test_sdk_table_structure(mcp_only_session: ClientSession):
    """sdk.mock is a table with function members."""
    parsed = await _exec(
        mcp_only_session,
        'return type(sdk.mock) .. ":" .. type(sdk.mock.echo)',
    )
    result = _unwrap(parsed)
    assert result == "table:function"


@pytest.mark.asyncio
async def test_multi_tool_chain(mcp_only_session: ClientSession):
    """Chain multiple MCP tool calls in a single script."""
    parsed = await _exec(
        mcp_only_session,
        """
local echo_result = sdk.mock.echo({ text = "chained" })
local add_result = sdk.mock.add({ a = 10, b = 20 })
-- Unwrap structured results if they are tables
local e = type(echo_result) == "table" and echo_result.result or echo_result
local a = type(add_result) == "table" and add_result.result or add_result
return tostring(e) .. "|" .. tostring(a)
""",
    )
    result = _unwrap(parsed)
    assert "chained" in result
    assert "30" in result
