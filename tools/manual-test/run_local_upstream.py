#!/usr/bin/env python3
"""Start the e2e test API (FastAPI) on a free local port for manual testing.

Wraps the same app the e2e suite uses, so journeys against a live upstream
exercise the real HTTP behavior (auth, seeding, CRUD) without network access.
The seeded bearer token is `test-secret-123` (see e2e/test_api/auth.py).

Usage:
    run_local_upstream.py

Prints the base URL on stdout. Keeps running until stdin closes or the process
is killed, so a driver can background it and read the URL first.
"""

import os
import socket
import sys
import threading
import time
from pathlib import Path

import uvicorn

E2E_DIR = Path(__file__).resolve().parents[2] / "e2e"
sys.path.insert(0, str(E2E_DIR))


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def main():
    port = free_port()
    url = f"http://127.0.0.1:{port}"
    os.environ["TEST_API_SERVER_URL"] = url
    import test_api.app as app_module

    config = uvicorn.Config(
        app_module.app,
        host="127.0.0.1",
        port=port,
        log_level="warning",
        access_log=False,
    )
    server = uvicorn.Server(config)
    threading.Thread(target=server.run, daemon=True).start()
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if server.started:
            break
        time.sleep(0.1)
    else:
        print("upstream failed to start", file=sys.stderr)
        sys.exit(1)
    print(url, flush=True)
    try:
        sys.stdin.read()  # keep alive until stdin closes
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
