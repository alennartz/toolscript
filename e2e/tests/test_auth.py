import json
import pytest
from mcp import ClientSession


def parse_result(result) -> dict:
    text = result.content[0].text
    assert not result.isError, f"Script execution error: {text}"
    return json.loads(text)


@pytest.mark.asyncio
async def test_no_auth_read_succeeds(mcp_no_auth_session: ClientSession):
    """Public endpoints work without any credentials."""
    result = await mcp_no_auth_session.call_tool("execute_script", {
        "script": "return sdk.list_pets()"
    })
    data = parse_result(result)
    assert data["result"] is not None
    assert data["result"]["total"] == 4


@pytest.mark.asyncio
async def test_no_auth_write_fails(mcp_no_auth_session: ClientSession):
    """Protected endpoints fail without credentials."""
    result = await mcp_no_auth_session.call_tool("execute_script", {
        "script": 'return sdk.create_pet({ name = "Fail", status = "active" })'
    })
    assert result.isError is True
    text = result.content[0].text
    assert "401" in text or "error" in text.lower() or "Unauthorized" in text


@pytest.mark.asyncio
async def test_bearer_token_auth(mcp_stdio_session: ClientSession):
    """Env-var bearer token allows mutations (uses main session with creds)."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": 'return sdk.create_pet({ name = "Bearer", status = "active" })'
    })
    data = parse_result(result)
    assert data["result"]["name"] == "Bearer"


@pytest.mark.asyncio
async def test_meta_auth_override(mcp_no_auth_session: ClientSession):
    """Passing _meta.auth with bearer token allows mutation on no-auth session."""
    # The Python MCP SDK's call_tool() accepts a `meta` keyword arg which maps
    # to the MCP protocol's _meta field in CallToolRequest.params.
    # code-mcp reads auth from context.request_context.meta.get("auth").
    result = await mcp_no_auth_session.call_tool(
        "execute_script",
        {"script": 'return sdk.create_pet({ name = "Meta", status = "active" })'},
        meta={
            "auth": {
                "test_api": {"type": "bearer", "token": "test-secret-123"}
            }
        },
    )
    data = parse_result(result)
    assert data["result"]["name"] == "Meta"
