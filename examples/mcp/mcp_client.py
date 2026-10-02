#!/usr/bin/env python3
"""mcp_client.py: talk to `openreadout mcp` over stdio without any MCP SDK.

Usage:   python examples/mcp/mcp_client.py FILE [--binary PATH]
Needs:   Python 3.9+ standard library only; the openreadout binary (or OPENREADOUT=PATH).
Output:  the MCP handshake, the tool list, and the results of openreadout_info,
         openreadout_check and a deliberately failing call, so you can see the shapes an
         agent receives.
Exit:    0 when every step behaved as expected, 1 otherwise (CI runs this).

This is what Claude Code, Cursor, Codex, Copilot or Gemini CLI do after you register the
server (`openreadout mcp --config <client>`): newline-delimited JSON-RPC 2.0 on the child's
stdin/stdout. Tool results are the same JSON the CLI prints as `data`, as text content; failures
are JSON-RPC errors whose `data` carries the CLI's `{code, exit_code, hint}`.
Based on oracle/mcp_smoke.py.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from typing import Any


class McpStdioClient:
    def __init__(self, argv: list[str]) -> None:
        self.proc = subprocess.Popen(
            argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1
        )
        self.next_id = 0

    def _send(self, msg: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def notify(self, method: str, params: dict[str, Any] | None = None) -> None:
        self._send({"jsonrpc": "2.0", "method": method, **({"params": params} if params is not None else {})})

    def request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        """Send a request and return the whole response (with `result` or `error`)."""
        self.next_id += 1
        msg: dict[str, Any] = {"jsonrpc": "2.0", "id": self.next_id, "method": method}
        if params is not None:
            msg["params"] = params
        self._send(msg)
        assert self.proc.stdout is not None
        while True:
            line = self.proc.stdout.readline()
            if not line:
                err = self.proc.stderr.read()[:2000] if self.proc.stderr else ""
                raise RuntimeError(f"server closed the connection; stderr: {err}")
            reply = json.loads(line)
            if reply.get("id") == self.next_id:  # skip server notifications, if any
                return reply

    def call_tool(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        return self.request("tools/call", {"name": name, "arguments": arguments})

    def close(self) -> int:
        assert self.proc.stdin is not None
        self.proc.stdin.close()  # end of input shuts the server down
        return self.proc.wait(timeout=10)


def tool_json(reply: dict[str, Any]) -> Any:
    """The JSON payload of a successful tool call (first text content block)."""
    return json.loads(reply["result"]["content"][0]["text"])


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("file")
    ap.add_argument("--binary", default=os.environ.get("OPENREADOUT", "openreadout"))
    args = ap.parse_args()
    failures = 0

    client = McpStdioClient([args.binary, "mcp"])
    init = client.request(
        "initialize",
        {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "example", "version": "0"}},
    )["result"]
    print(f"server: {init['serverInfo']['name']} {init['serverInfo'].get('version', '')}, protocol {init['protocolVersion']}")
    client.notify("notifications/initialized")

    tools = client.request("tools/list")["result"]["tools"]
    print("tools:")
    for t in tools:
        print(f"  {t['name']:<24} {t.get('description', '').splitlines()[0][:70]}")

    info = tool_json(client.call_tool("openreadout_info", {"file": args.file}))
    print(f"\nopenreadout_info -> {info['format']['name']}, {len(info['images'])} image(s), {info['plane_count']} plane(s)")
    for im in info["images"][:5]:
        print(f"  [{im['index']}] {im['size_x']}x{im['size_y']} z={im['size_z']} c={im['size_c']} t={im['size_t']} {im['pixel_type']}")

    check = tool_json(client.call_tool("openreadout_check", {"file": args.file}))
    errors = [f for f in check["findings"] if f["severity"] == "error"]
    print(f"\nopenreadout_check -> ok={check['ok']}, {len(errors)} error finding(s)")

    # A failing call: errors come back as JSON-RPC errors (or isError results) an agent can act on.
    reply = client.call_tool("openreadout_info", {"file": "/definitely/not/here.czi"})
    if "error" in reply:
        err = reply["error"]
        print(f"\nexpected failure -> JSON-RPC error {err['code']}: {err['message']}; data={json.dumps(err.get('data'))}")
    elif reply.get("result", {}).get("isError"):
        print(f"\nexpected failure -> isError result: {reply['result']['content'][0]['text'][:200]}")
    else:
        print("\nerror: a missing file did not produce an error", file=sys.stderr)
        failures += 1

    schema = next(t for t in tools if t["name"] == "openreadout_search").get("outputSchema", {})
    print(f"\nopenreadout_search output schema -> {len(schema.get('properties', {}))} top-level properties")

    code = client.close()
    print(f"server exited with {code}")
    return 1 if failures or code != 0 else 0


if __name__ == "__main__":
    sys.exit(main())
