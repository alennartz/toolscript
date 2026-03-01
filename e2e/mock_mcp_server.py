"""Standalone mock MCP server for e2e testing.

Provides diverse tool schemas (string, int, bool, optional, no-params, error)
to exercise the full type conversion and execution pipeline.

Usage: python mock_mcp_server.py   (runs over stdio)
"""

from mcp.server.fastmcp import FastMCP

mcp = FastMCP("mock-tools")


@mcp.tool()
def echo(text: str) -> str:
    """Echo back the input text."""
    return text


@mcp.tool()
def add(a: int, b: int) -> int:
    """Add two numbers together."""
    return a + b


@mcp.tool()
def get_user(user_id: str, include_email: bool = False) -> dict:
    """Look up a user by ID."""
    user = {"id": user_id, "name": "Alice"}
    if include_email:
        user["email"] = "alice@example.com"
    return user


@mcp.tool()
def list_items(category: str, limit: int = 10) -> list[dict]:
    """List items in a category."""
    return [{"id": str(i), "name": f"{category}-{i}"} for i in range(min(limit, 3))]


@mcp.tool()
def failing_tool() -> str:
    """A tool that always fails."""
    raise ValueError("intentional failure")


@mcp.tool()
def no_params() -> str:
    """A tool with no parameters."""
    return "ok"


if __name__ == "__main__":
    mcp.run()
