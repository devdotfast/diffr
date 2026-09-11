#!/usr/bin/env python3
"""Minimal JSON-RPC 2.0 fold hook for tests, standard library only.

Listens on 127.0.0.1:$DIFFR_HOOK_PORT. The first argument selects a behavior:
  echo   answer every fold with "pseudo <placeholder>"
  first  answer only the first fold with "summary of f"
  error  return a JSON-RPC error for every request
  slow   never answer (sleeps inside the handler)
  cwd    answer the first fold with the working directory and assert DIFFR_WORKSPACE
  bad    return a non-JSON body
"""

import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODE = sys.argv[1]


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        assert request["jsonrpc"] == "2.0" and request["method"] == "summarize", request
        params = request["params"]
        assert params["src"] and isinstance(params["folds"], list), params
        if MODE == "slow":
            time.sleep(30)
        if MODE == "bad":
            body = b"not json"
        elif MODE == "error":
            body = json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "error": {"code": -32000, "message": "declined"},
                }
            ).encode()
        else:
            if MODE == "echo":
                texts = {
                    str(f["id"]): "pseudo " + f["placeholder"] for f in params["folds"]
                }
            elif MODE == "first":
                texts = {str(params["folds"][0]["id"]): "summary of f"}
            elif MODE == "cwd":
                assert os.environ["DIFFR_WORKSPACE"] == sys.argv[2], os.environ[
                    "DIFFR_WORKSPACE"
                ]
                texts = {str(params["folds"][0]["id"]): os.getcwd()}
            else:
                raise SystemExit(f"unknown mode {MODE}")
            body = json.dumps(
                {"jsonrpc": "2.0", "id": request["id"], "result": texts}
            ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


if MODE == "exit":
    raise SystemExit(3)
ThreadingHTTPServer(
    ("127.0.0.1", int(os.environ["DIFFR_HOOK_PORT"])), Handler
).serve_forever()
