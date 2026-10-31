#!/usr/bin/env python3
"""Recorre las herramientas MCP de odotui contra un Odoo real.

Pensado para dev/docker-compose.yml:

    docker compose -f dev/docker-compose.yml up -d
    cargo build && python3 scripts/mcp_smoke.py

Crea la BD `odotui_smoke` (y copias con ese prefijo) y la borra al final
salvo con --keep. Solo usa la biblioteca estándar.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


class Mcp:
    """Cliente MCP por stdio, un mensaje JSON por línea."""

    def __init__(self, cmd):
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, cwd=ROOT)
        self.n = 0

    def send(self, method, params=None, notify=False):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if not notify:
            self.n += 1
            msg["id"] = self.n
        self.p.stdin.write(json.dumps(msg) + "\n")
        self.p.stdin.flush()
        if notify:
            return None
        # Las notificaciones (p. ej. de progreso) llegan antes de la respuesta.
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError("odotui mcp se cerró")
            r = json.loads(line)
            if r.get("id") == msg["id"]:
                return r
            if r.get("method") == "notifications/progress":
                self.progress = r["params"]

    def call(self, name, args):
        self.progress = None
        r = self.send("tools/call", {"name": name, "arguments": args, "_meta": {"progressToken": name}})
        res = r["result"]
        return res["isError"], res["content"][0]["text"]

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=30)


def wait_http(url, seconds=90):
    end = time.time() + seconds
    while time.time() < end:
        try:
            urllib.request.urlopen(url + "/web/login", timeout=5)
            return True
        except Exception:
            time.sleep(2)
    return False


def login(url, db, user, password):
    body = json.dumps({"jsonrpc": "2.0", "params": {"db": db, "login": user, "password": password}}).encode()
    req = urllib.request.Request(url + "/web/session/authenticate", body, {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        data = json.load(r)
    return bool(data.get("result", {}).get("uid"))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-c", "--config", default="dev/odotui.toml")
    ap.add_argument("--bin", default=str(ROOT / "target/debug/odotui"))
    ap.add_argument("--db", default="odotui_smoke")
    ap.add_argument("--url", default="http://localhost:8069")
    ap.add_argument("--keep", action="store_true", help="no borrar las BDs al final")
    ap.add_argument("--allow-tui", action="store_true", help="dejar que las llamadas vayan a una TUI abierta")
    a = ap.parse_args()

    sock = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "odotui.sock"
    if sock.exists() and not a.allow_tui:
        sys.exit(f"hay una TUI de odotui abierta ({sock}): las llamadas irían a ella. Ciérrala o usa --allow-tui.")

    db, copy = a.db, a.db + "_copia"
    restored = a.db + "_restaurada"
    m = Mcp([a.bin, "mcp", "-c", a.config])
    init = m.send("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "mcp_smoke", "version": "1"}})
    m.send("notifications/initialized", notify=True)
    tools = {t["name"] for t in m.send("tools/list")["result"]["tools"]}
    print(f"odotui {init['result']['serverInfo']['version']}: {len(tools)} herramientas")

    fails = []

    def step(name, args, expect=None, error=False, show=False):
        if name not in tools:
            print(f"  · {name}: no existe en esta versión")
            return None
        t0 = time.time()
        is_err, text = m.call(name, args)
        ok = is_err == error and (expect is None or expect in text)
        mark = "✓" if ok else "✗"
        extra = f" (progreso: {m.progress['progress']:.0f}/{m.progress.get('total', '?')})" if m.progress else ""
        print(f"{mark} {name} {json.dumps(args, ensure_ascii=False)[:90]}  {time.time() - t0:.1f}s{extra}")
        if not ok or show:
            print("    " + text[:2000].replace("\n", "\n    "))
        if not ok:
            fails.append(name)
        return text

    step("status", {}, expect="Perfil activo")
    for d in (db, copy, restored):
        step("drop_database", {"database": d})
    # `-i base` crea la BD si no existe.
    step("install_modules", {"database": db, "modules": ["base"]})
    step("list_databases", {}, expect=db)
    step("install_modules", {"database": db, "modules": ["odotui_demo"]})
    step("list_modules", {"database": db, "filter": "odotui"}, expect="odotui_demo\tinstalled")
    step("odoo_shell", {"database": db, "code": "n = env['odotui.nota'].create({'name': 'smoke', 'texto': 'hola'}); env.cr.commit(); print('largo', n.largo)"}, expect="largo 4")
    step("sql_query", {"database": db, "sql": "SELECT name, largo FROM odotui_nota"}, expect="smoke")
    step("sql_query", {"database": db, "sql": "DELETE FROM odotui_nota"}, error=True)
    step("run_tests", {"database": db, "modules": ["odotui_demo"]}, expect="OK")
    step("update_modules", {"database": db, "modules": ["odotui_demo"]})
    step("describe_model", {"database": db, "model": "odotui.nota"}, expect="largo")
    step("find_views", {"database": db, "model": "odotui.nota"}, expect="odotui_demo.nota_view_form")
    step("get_view", {"database": db, "xml_id": "odotui_demo.nota_view_form"}, expect="partner_id")
    step("module_info", {"database": db, "name": "odotui_demo"}, expect="17.0.1.0.0")
    step("search_records", {"database": db, "model": "odotui.nota", "fields": ["name", "largo"]}, expect="smoke")
    step("neutralize_database", {"database": db})
    step("sql_query", {"database": db, "sql": "SELECT value FROM ir_config_parameter WHERE key = 'database.is_neutralized'"}, expect="true")
    step("duplicate_database", {"source": db, "target": copy})
    step("backup_database", {"database": db, "format": "zip"}, expect=".zip")
    backups = step("list_backups", {}, expect=db) or ""
    zips = [l.split("\t")[0] for l in backups.splitlines() if l.startswith(db) and ".zip" in l]
    if zips:
        step("restore_backup", {"backup": zips[0], "database": restored}, expect="restaurada")
        step("sql_query", {"database": restored, "sql": "SELECT count(*) FROM odotui_nota"}, expect="1")
    step("uninstall_modules", {"database": copy, "modules": ["odotui_demo"]})
    step("list_modules", {"database": copy, "filter": "odotui"}, expect="uninstalled")
    step("scaffold_module", {"name": "odotui_smoke_scaffold"}, expect="odotui_smoke_scaffold")
    shutil.rmtree(ROOT / "dev/addons/odotui_smoke_scaffold", ignore_errors=True)
    step("instance", {"action": "restart"})
    step("read_logs", {"lines": 50})
    step("read_logs", {"errors_only": True})

    if wait_http(a.url):
        ok = login(a.url, db, "admin", "admin")
        print(f"{'✓' if ok else '✗'} login admin/admin en {a.url} ({db})")
        if not ok:
            fails.append("login")
    else:
        print(f"✗ {a.url} no responde")
        fails.append("http")

    if not a.keep:
        for d in (db, copy, restored):
            step("drop_database", {"database": d})
        for z in zips:
            (ROOT / "dev/backups" / z).unlink(missing_ok=True)
    m.close()
    print(f"\n{'✓ todo bien' if not fails else '✗ fallaron: ' + ', '.join(fails)}")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
