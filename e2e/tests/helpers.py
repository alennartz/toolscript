"""Shared test helpers for e2e tests."""

import json

from mcp import ClientSession


async def exec_script(session: ClientSession, script: str):
    """Call execute_script and return the parsed JSON response."""
    result = await session.call_tool("execute_script", {"script": script})
    return json.loads(result.content[0].text)


def unwrap(parsed):
    """Unwrap the execute_script result.

    MCP tools using structured_content return {"result": value} as a table.
    Plain text results return strings directly.
    """
    r = parsed["result"]
    if isinstance(r, dict) and "result" in r:
        return r["result"]
    return r
