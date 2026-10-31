#!/usr/bin/env python3
"""Llama a una herramienta de odotui por MCP (stdio) cuando la sesión no tiene
las herramientas `mcp__odotui__*` cargadas.

    llamar.py status
    llamar.py list_modules '{"database": "prueba", "state": "installed"}'

Si la TUI está abierta, la llamada corre dentro de ella (Jorge la ve en vivo).
Sale con código 1 si la herramienta devolvió error.
"""

import json
import os
import shutil
import subprocess
import sys


def main():
    if len(sys.argv) < 2 or sys.argv[1] in ("-h", "--help"):
        sys.exit(__doc__)
    tool = sys.argv[1]
    args = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
    exe = shutil.which("odotui") or os.path.expanduser("~/.cargo/bin/odotui")
    p = subprocess.Popen([exe, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

    def rpc(i, method, params):
        p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": method, "params": params}) + "\n")
        p.stdin.flush()
        # Puede haber notificaciones de avance antes de la respuesta.
        while True:
            msg = json.loads(p.stdout.readline())
            if msg.get("id") == i:
                return msg

    rpc(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "skill-odotui", "version": "1"}})
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
    r = rpc(2, "tools/call", {"name": tool, "arguments": args})
    p.stdin.close()
    p.wait()
    if "error" in r:
        sys.exit(f"error MCP: {r['error'].get('message')}")
    res = r["result"]
    print("\n".join(c.get("text", "") for c in res.get("content", [])))
    sys.exit(1 if res.get("isError") else 0)


if __name__ == "__main__":
    main()
