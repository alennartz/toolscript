#!/usr/bin/env python3
"""Minimal MCP stdio client for manual testing of toolscript.

Speaks JSON-RPC 2.0 over the MCP stdio transport (line-delimited JSON) with a
spawned server process. Supports initialize, tools/list, tools/call, and
resources/read — enough to exercise every toolscript MCP surface.

Usage:
    mcp_stdio.py <toolscript-cmd...> [--script-file <path>] [--timeout <sec>]

Reads a scenario from stdin, one JSON object per line:
    {"op": "list_apis"}
    {"op": "list_functions", "api": "petstore"}          # optional filter
    {"op": "get_function_docs", "name": "list_pets"}
    {"op": "search_docs", "query": "..."}
    {"op": "execute_script", "script": "return 1 + 1"}
    {"op": "read_resource", "uri": "sdk://petstore/overview"}

Prints one JSON result object per line to stdout. Exit code 1 if any op
returns an error or the server fails.
"""

import json
import subprocess
import sys
import time

MCP_VERSION = "2024-11-05"


class McpStdioClient:
    def __init__(self, cmd):
        self.proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        self.next_id = 0

    def _send(self, method, params=None, notify=False):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if not notify:
            self.next_id += 1
            msg["id"] = self.next_id
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()
        return self.next_id

    def _recv(self):
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("server closed stdout")
            line = line.strip()
            if not line:
                continue
            msg = json.loads(line)
            if "id" in msg:
                return msg

    def request(self, method, params=None):
        rid = self._send(method, params)
        while True:
            msg = self._recv()
            if msg.get("id") == rid:
                if "error" in msg:
                    raise RuntimeError(f"{method} error: {msg['error']}")
                return msg["result"]

    def initialize(self):
        result = self.request(
            "initialize",
            {
                "protocolVersion": MCP_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "manual-test", "version": "0.0.1"},
            },
        )
        self._send("notifications/initialized", notify=True)
        # rmcp settles the handshake asynchronously; give it a moment so the
        # first real request is not raced.
        time.sleep(0.3)
        return result

    def call_tool(self, name, arguments):
        return self.request("tools/call", {"name": name, "arguments": arguments})

    def read_resource(self, uri):
        return self.request("resources/read", {"uri": uri})

    def close(self):
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        try:
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


def main():
    args = sys.argv[1:]
    timeout = 60
    if "--timeout" in args:
        i = args.index("--timeout")
        timeout = int(args[i + 1])
        del args[i : i + 2]
    cmd = args

    scenarios = [json.loads(line) for line in sys.stdin if line.strip()]
    client = McpStdioClient(cmd)
    failures = 0
    try:
        client.initialize()
        for step in scenarios:
            op = step["op"]
            try:
                if op == "list_apis":
                    result = client.call_tool("list_apis", {})
                elif op == "list_functions":
                    arguments = {}
                    if "api" in step:
                        arguments["api"] = step["api"]
                    result = client.call_tool("list_functions", arguments)
                elif op == "get_function_docs":
                    result = client.call_tool(
                        "get_function_docs", {"name": step["name"]}
                    )
                elif op == "search_docs":
                    result = client.call_tool("search_docs", {"query": step["query"]})
                elif op == "execute_script":
                    result = client.call_tool("execute_script", {"script": step["script"]})
                elif op == "read_resource":
                    result = client.read_resource(step["uri"])
                else:
                    raise ValueError(f"unknown op: {op}")
            except RuntimeError as exc:
                result = {"ERROR": str(exc)}
                failures += 1
            print(json.dumps({"op": op, **step, "result": result}))
    finally:
        client.close()
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
