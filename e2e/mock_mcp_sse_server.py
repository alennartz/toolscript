"""Mock MCP server that serves over SSE transport for e2e testing.

Uses FastMCP's sse_app() to create a Starlette ASGI app that speaks
the legacy SSE transport, then runs it with uvicorn.

Usage: python mock_mcp_sse_server.py <port>
"""

import sys

import uvicorn
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("sse-mock-tools")


@mcp.tool()
def echo(text: str) -> str:
    """Echo back the input text."""
    return text


@mcp.tool()
def add(a: int, b: int) -> int:
    """Add two numbers together."""
    return a + b


@mcp.tool()
def no_params() -> str:
    """A tool with no parameters."""
    return "ok"


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    app = mcp.sse_app()
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning")
