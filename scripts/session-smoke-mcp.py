#!/usr/bin/env python3
"""MCP App UI smoke for session-close (unattended).

Talks to the installed (or SWERVE_MCP_EXE) MCP sidecar over stdio JSON-RPC.
Requires a running Swerve Build with App UI grant + CDP (see session-smoke.ps1).

Exit codes:
  0 — all steps passed
  1 — one or more steps failed
  2 — MCP binary missing / cannot start
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import threading
from typing import Any


def resolve_mcp() -> str:
    env = os.environ.get("SWERVE_MCP_EXE")
    if env and os.path.isfile(env):
        return env
    local = os.environ.get("LOCALAPPDATA", "")
    candidates = [
        os.path.join(local, "Swerve Build", "swervebuild-mcp.exe"),
        os.path.join(local, "SwerveBuild", "swervebuild-mcp.exe"),
    ]
    for c in candidates:
        if os.path.isfile(c):
            return c
    return candidates[0]


def call_tool(mcp: str, name: str, arguments: dict | None = None, timeout: float = 20.0) -> dict[str, Any]:
    """One-shot MCP process so a hung tool cannot block the whole smoke."""
    if not os.path.isfile(mcp):
        return {"error": f"mcp missing: {mcp}", "fatal": True}

    proc = subprocess.Popen(
        [mcp],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        encoding="utf-8",
        bufsize=1,
    )
    result: dict[str, Any] = {"error": "timeout"}

    def run() -> None:
        nonlocal result
        try:
            def rpc(req_id: int, method: str, params: dict | None = None) -> dict:
                msg: dict[str, Any] = {"jsonrpc": "2.0", "id": req_id, "method": method}
                if params is not None:
                    msg["params"] = params
                assert proc.stdin is not None and proc.stdout is not None
                proc.stdin.write(json.dumps(msg) + "\n")
                proc.stdin.flush()
                while True:
                    out = proc.stdout.readline()
                    if not out:
                        raise RuntimeError("EOF from MCP")
                    data = json.loads(out)
                    if data.get("id") == req_id:
                        return data

            rpc(
                1,
                "initialize",
                {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "session-smoke", "version": "1"},
                },
            )
            assert proc.stdin is not None
            proc.stdin.write(
                json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"
            )
            proc.stdin.flush()
            r = rpc(2, "tools/call", {"name": name, "arguments": arguments or {}})
            content = (r.get("result") or {}).get("content") or [{}]
            text = content[0].get("text", "") if content else ""
            # Surface structured isError when present
            is_err = (r.get("result") or {}).get("isError")
            if is_err is None and r.get("error"):
                is_err = True
                text = text or json.dumps(r.get("error"))
            # Heuristic: tool text often embeds "error" JSON
            lower = (text or "").lower()
            if is_err is None and (
                '"ok":false' in lower.replace(" ", "")
                or "not granted" in lower
                or "unauthorized" in lower
                or "cdp" in lower and "not" in lower and "ready" in lower
            ):
                is_err = True
            result = {"isError": bool(is_err), "text": text}
        except Exception as e:  # noqa: BLE001 — smoke must never crash the runner silently
            result = {"error": str(e)}
        finally:
            try:
                proc.kill()
            except Exception:
                pass

    t = threading.Thread(target=run, daemon=True)
    t.start()
    t.join(timeout)
    if t.is_alive():
        try:
            proc.kill()
        except Exception:
            pass
        return {"error": f"timeout after {timeout}s"}
    return result


def step_ok(r: dict[str, Any]) -> bool:
    if r.get("fatal"):
        return False
    if r.get("error"):
        return False
    if r.get("isError"):
        return False
    return True


# Profiles: keep core short enough for every product session close.
CORE_STEPS: list[tuple[str, dict, float]] = [
    ("get_app_status", {}, 10),
    ("app_ui_state", {}, 15),
    ("app_ui_screenshot", {}, 25),
    ("app_ui_click", {"selector": "a[href='/settings']"}, 25),
    ("app_ui_wait", {"condition": "route:/settings", "timeout_ms": 8000}, 20),
    ("app_ui_state", {}, 15),
    ("app_ui_click", {"selector": "a[href='/']"}, 25),
    ("app_ui_wait", {"condition": "route:/", "timeout_ms": 8000}, 20),
    ("app_ui_screenshot", {}, 25),
]

# Deeper drive (type/press) — optional; can leave dirty settings field if fail mid-way.
DEEP_STEPS: list[tuple[str, dict, float]] = [
    ("app_ui_click", {"selector": "a[href='/settings']"}, 25),
    ("app_ui_wait", {"condition": "route:/settings", "timeout_ms": 5000}, 20),
    ("app_ui_wait", {"condition": ".ids-row input", "timeout_ms": 5000}, 20),
    ("app_ui_type", {"selector": ".ids-row input", "text": "smoke-session-typed"}, 25),
    ("app_ui_type", {"selector": ".ids-row input", "text": ""}, 25),
    ("app_ui_click", {"selector": "a[href='/']"}, 25),
    ("app_ui_wait", {"condition": "route:/", "timeout_ms": 5000}, 20),
]


def main() -> int:
    ap = argparse.ArgumentParser(description="SwerveBuild MCP session smoke")
    ap.add_argument(
        "--profile",
        choices=("core", "deep"),
        default="core",
        help="core = status/nav/screenshot; deep adds type into settings",
    )
    args = ap.parse_args()
    mcp = resolve_mcp()
    if not os.path.isfile(mcp):
        print(f"FAIL: MCP binary not found: {mcp}", file=sys.stderr)
        return 2

    steps = list(CORE_STEPS)
    if args.profile == "deep":
        steps.extend(DEEP_STEPS)

    print(f"MCP: {mcp}")
    print(f"profile: {args.profile} ({len(steps)} steps)")
    failed = 0
    for tool, tool_args, timeout in steps:
        print(f"\n=== {tool} {tool_args} ===")
        sys.stdout.flush()
        r = call_tool(mcp, tool, tool_args, timeout=timeout)
        preview = json.dumps(r, indent=2)
        if len(preview) > 1500:
            preview = preview[:1500] + "…"
        print(preview)
        sys.stdout.flush()
        if not step_ok(r):
            failed += 1
            print(f"FAIL step: {tool}")
        else:
            print(f"OK step: {tool}")

    if failed:
        print(f"\nDONE — {failed} failed step(s)")
        return 1
    print("\nDONE — all steps passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
