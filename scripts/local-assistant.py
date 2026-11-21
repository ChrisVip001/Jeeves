#!/usr/bin/env python3
"""Launch a loopback Jeeves workspace and persist only its own account and Runtime state."""
import json
import os
import secrets
import signal
import socket
import subprocess
import time
import urllib.request
import webbrowser
from pathlib import Path

root = Path(__file__).resolve().parents[1]
home = Path(os.environ.get("JEEVES_PERSONAL_HOME", Path.home() / ".local/share/jeeves-personal"))
home.mkdir(mode=0o700, parents=True, exist_ok=True)
if home.stat().st_mode & 0o077:
    raise SystemExit("The personal workspace directory must be private (mode 0700)")
settings = home / "gateway-secrets.json"
if not settings.exists():
    keys = {name: secrets.token_hex(32) for name in ("GW_SESSION_KEY", "GW_STATE_KEY", "GW_SETUP_TOKEN", "JEEVES_OPS_TOKEN")}
    fd = os.open(settings, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as stream:
        json.dump(keys, stream)
keys = json.loads(settings.read_text())
with socket.socket() as reservation:
    reservation.bind(("127.0.0.1", 0))
    runtime_port = reservation.getsockname()[1]
gateway_port = int(os.environ.get("JEEVES_PERSONAL_PORT", "7788"))
origin = f"http://127.0.0.1:{gateway_port}"
runtime_origin = f"http://127.0.0.1:{runtime_port}"
asset = root / "jeeves-ts/packages/dashboard/dist"
runtime = subprocess.Popen(
    [root / "jeeves-rs/target/debug/jeeves", "dashboard"],
    cwd=root,
    env={
        **os.environ, "JEEVES_HOME": str(home / "runtime"), "JEEVES_OPS_HOST": "127.0.0.1",
        "JEEVES_PORT": str(runtime_port), "JEEVES_DASHBOARD_DIR": str(asset),
        "JEEVES_OPS_TOKEN": keys["JEEVES_OPS_TOKEN"],
    },
)
gateway = None

def stop(_signal, _frame):
    raise KeyboardInterrupt

signal.signal(signal.SIGTERM, stop)

def await_health(service, started):
    for _ in range(150):
        if started.poll() is not None:
            raise SystemExit("Jeeves failed during startup")
        try:
            with urllib.request.urlopen(service + "/healthz", timeout=1) as result:
                if result.status == 200:
                    return
        except OSError:
            time.sleep(.2)
    raise SystemExit("Jeeves did not become ready")

try:
    await_health(runtime_origin, runtime)
    gateway = subprocess.Popen(
        [root / "jeeves-rs/target/jeeves-gateway"],
        cwd=root,
        env={
            **{k: v for k, v in os.environ.items() if not k.startswith("GW_")},
            **{k: v for k, v in keys.items() if k.startswith("GW_")},
            "GW_LOCAL": "1", "GW_ADDR": f"127.0.0.1:{gateway_port}",
            "GW_PUBLIC_ORIGIN": origin, "GW_RUNTIME_URL": runtime_origin,
            "GW_RUNTIME_TOKEN": keys["JEEVES_OPS_TOKEN"],
            "GW_DATABASE_URL": "sqlite:" + str(home / "accounts.sqlite"),
            "GW_DASHBOARD_DIR": str(asset),
        },
    )
    await_health(origin, gateway)
    import sqlite3
    with sqlite3.connect((home / "accounts.sqlite").as_uri() + "?mode=ro", uri=True) as db:
        created = db.execute("SELECT EXISTS(SELECT 1 FROM gw_users)").fetchone()[0]
    entry = origin + "/"
    if not created:
        entry = origin + "/account/?next=%2Fapp%2F#setup=" + keys["GW_SETUP_TOKEN"]
    print(f"Jeeves is running at {origin}", flush=True)
    if os.environ.get("JEEVES_PERSONAL_BROWSER") != "0":
        webbrowser.open(entry)
    gateway.wait()
except KeyboardInterrupt:
    pass
finally:
    for child in (gateway, runtime):
        if child is not None and child.poll() is None:
            child.terminate()
    for child in (gateway, runtime):
        if child is None:
            continue
        try:
            child.wait(timeout=30)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
