import json
import os
import subprocess
import sys
import threading

# Override with SWERVE_MCP_EXE to smoke a dev build (e.g. target/debug/swervebuild_mcp.exe).
mcp = os.environ.get("SWERVE_MCP_EXE") or os.path.join(
    os.environ.get("LOCALAPPDATA", ""), "Swerve Build", "swervebuild-mcp.exe"
)


def call_tool(name, arguments=None, timeout=20):
    """One-shot MCP process so a hung tool cannot block the whole smoke."""
    proc = subprocess.Popen(
        [mcp],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        bufsize=1,
    )
    result = {"error": "timeout"}

    def run():
        nonlocal result
        try:
            def rpc(req_id, method, params=None):
                msg = {"jsonrpc": "2.0", "id": req_id, "method": method}
                if params is not None:
                    msg["params"] = params
                proc.stdin.write(json.dumps(msg) + "\n")
                proc.stdin.flush()
                while True:
                    out = proc.stdout.readline()
                    if not out:
                        raise RuntimeError("EOF")
                    data = json.loads(out)
                    if data.get("id") == req_id:
                        return data

            rpc(1, "initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "smoke", "version": "0"},
            })
            proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
            proc.stdin.flush()
            r = rpc(2, "tools/call", {"name": name, "arguments": arguments or {}})
            content = (r.get("result") or {}).get("content") or [{}]
            text = content[0].get("text", "") if content else ""
            result = {
                "isError": (r.get("result") or {}).get("isError"),
                "text": text,
            }
        except Exception as e:
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


for tool, args, timeout in [
    ("get_app_status", {}, 10),
    ("list_projects", {}, 10),
    ("app_ui_state", {}, 15),
    ("app_ui_snapshot", {}, 15),
    ("app_ui_screenshot", {}, 25),
    ("app_ui_click", {"selector": "a[href='/settings']"}, 25),
    ("app_ui_state", {}, 15),
]:
    print(f"\n=== {tool} {args} ===")
    sys.stdout.flush()
    r = call_tool(tool, args, timeout=timeout)
    print(json.dumps(r, indent=2)[:2000])
    sys.stdout.flush()

print("\nDONE")
