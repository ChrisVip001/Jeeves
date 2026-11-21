#!/usr/bin/env python3
"""Real process lifecycle regression: restart, drain SSE, and reject a duplicate home.

Uses the built Runtime and Go gateway with ephemeral homes, ports and accounts.
"""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
import urllib.error
import urllib.request

from gateway_fixture import gateway

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "jeeves-rs/target/debug/jeeves"
TOKEN = "lifecycle-fixture-only"


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def start(args, home, http_port):
    env = {k: v for k, v in os.environ.items() if not k.startswith("JEEVES_")}
    env.update(JEEVES_HOME=home, JEEVES_PORT=str(http_port), JEEVES_PROVIDER="ollama",
               JEEVES_OPS_TOKEN=TOKEN, JEEVES_OPS_HOST="127.0.0.1",
               JEEVES_SEARCH_MCP="0", JEEVES_MCP_CONFIG="")
    return subprocess.Popen([str(BIN), *args], env=env, stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)


def cleanup(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=4)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=4)
    if process.stdin:
        process.stdin.close()
    if process.stdout:
        process.stdout.close()


def health(http_port):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{http_port}/healthz", timeout=2) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code
    except OSError:
        return None


def wait_healthy(http_port):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if health(http_port) == 200:
            return
        time.sleep(.1)
    raise AssertionError("fixture did not become ready")


def open_sse(http_port, path):
    return urllib.request.urlopen(urllib.request.Request(
        f"http://127.0.0.1:{http_port}{path}",
        headers={"Authorization": f"Bearer {TOKEN}", "Accept": "text/event-stream"}), timeout=5)


def stops_with_streams_open(signal_kind=signal.SIGTERM):
    with tempfile.TemporaryDirectory(prefix="jeeves-lifecycle-stop-") as home:
        http_port = port()
        dashboard = start(["runtime"], home, http_port)
        streams = []
        try:
            wait_healthy(http_port)
            streams = [open_sse(http_port, "/api/events"), open_sse(http_port, "/api/notifications")]
            # TERM targets the parent as launchd/Docker would; INT targets the terminal
            # process group. Both must drain the app-server, not kill it in place.
            if signal_kind == signal.SIGINT:
                os.killpg(dashboard.pid, signal_kind)
            else:
                dashboard.send_signal(signal_kind)
            assert dashboard.wait(timeout=12) == 0, "dashboard failed to shut down cleanly"
            assert health(http_port) is None, "listener is still open"
            assert not (Path(home) / "app-server.lock").exists(), "app-server lock was not released"
            print(f"PASS {signal_kind.name} with long SSE connections still open", flush=True)
        finally:
            for stream in streams:
                stream.close()
            cleanup(dashboard)


def ctrl_c_with_streams_open():
    stops_with_streams_open(signal.SIGINT)


def gateway_follows_restarted_runtime():
    class UnrelatedServer(BaseHTTPRequestHandler):
        def do_HEAD(self):
            self.send_response(204)
            self.end_headers()

        def do_GET(self):
            raise AssertionError("The gateway routed an API request to the old port")

        def log_message(self, *_):
            pass

    with tempfile.TemporaryDirectory(prefix="jeeves-dynamic-runtime-") as tmp:
        home = Path(tmp)
        endpoint = home / "runtime-endpoint.json"
        with gateway(home, home, TOKEN) as (_, workspace, cookie):
            def api(path, payload=None):
                request = urllib.request.Request(
                    workspace + "api/" + path,
                    data=json.dumps(payload).encode() if payload is not None else None,
                    headers={"Cookie": cookie, "Content-Type": "application/json"},
                )
                with urllib.request.urlopen(request, timeout=3) as response:
                    return json.load(response)

            def await_endpoint(previous=None):
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    if endpoint.is_file():
                        record = json.loads(endpoint.read_text())
                        if record != previous and api("workspace")["ready"]:
                            return record
                    time.sleep(.1)
                raise AssertionError("Go did not discover the Runtime")

            assert not api("workspace")["ready"], "the gateway started before the Runtime"
            runtime = start(["runtime"], str(home), 0)
            try:
                first = await_endpoint()
                task = api("task", {"action": "create", "sessionId": "jeeves-main", "title": "Persistent fixture task"})
                task_id = task["task"]["id"]
            finally:
                cleanup(runtime)
            with ThreadingHTTPServer(("127.0.0.1", first["port"]), UnrelatedServer) as reused:
                thread = Thread(target=reused.serve_forever, daemon=True)
                thread.start()
                try:
                    assert not api("workspace")["ready"]
                    try:
                        api("tasks")
                    except urllib.error.HTTPError as error:
                        assert error.code == 503
                    else:
                        raise AssertionError("The gateway accepted a stale endpoint")
                    runtime = start(["runtime"], str(home), 0)
                    try:
                        second = await_endpoint(first)
                        assert second["port"] != first["port"]
                        assert task_id in {row["id"] for row in api("tasks")["tasks"]}
                    finally:
                        cleanup(runtime)
                finally:
                    reused.shutdown()
                    thread.join(timeout=2)
            print("PASS authenticated Go workspace followed the new Runtime port and kept the original task", flush=True)


def refuses_duplicate_home():
    with tempfile.TemporaryDirectory(prefix="jeeves-lifecycle-conflict-") as home:
        first_port, second_port = port(), port()
        while second_port == first_port:
            second_port = port()
        first = start(["runtime"], home, first_port)
        second = None
        try:
            wait_healthy(first_port)
            second = start(["runtime"], home, second_port)
            # Observe the port during startup, not just after exit: the conflicting shell
            # must never publish a healthy-looking listener while the engine is refusing it.
            deadline = time.monotonic() + 12
            while second.poll() is None and time.monotonic() < deadline:
                assert health(second_port) is None, "duplicate dashboard opened its port"
                time.sleep(.1)
            assert second.wait(timeout=1) != 0, "duplicate runtime did not fail"
            output = second.stdout.read()
            assert "Jeeves Runtime →".encode() not in output, "duplicate advertised a Runtime URL"
            assert b"Lark gateway started" not in output, "duplicate started a gateway"
            assert health(first_port) == 200, "existing runtime was affected"
            assert health(second_port) is None, "duplicate listener remains"
            print("PASS duplicate home rejected before listening; first runtime unaffected", flush=True)
        finally:
            if second:
                cleanup(second)
            cleanup(first)


def exits_when_engine_crashes():
    with tempfile.TemporaryDirectory(prefix="jeeves-lifecycle-child-") as home:
        http_port = port()
        dashboard = start(["runtime"], home, http_port)
        stream = None
        restarted = None
        try:
            wait_healthy(http_port)
            stream = open_sse(http_port, "/api/events")
            child_pid = int((Path(home) / "app-server.lock").read_text())
            # The lock is in this unique fixture home; this is not the user's runtime.
            os.kill(child_pid, signal.SIGKILL)
            assert dashboard.wait(timeout=12) != 0, "cockpit survived an engine crash"
            assert health(http_port) is None, "failed cockpit still listens"
            restarted = start(["runtime"], home, http_port)
            wait_healthy(http_port)
            print("PASS engine crash closes cockpit; stale lock recovers on the next start", flush=True)
        finally:
            if stream:
                stream.close()
            if restarted:
                cleanup(restarted)
            cleanup(dashboard)


if __name__ == "__main__":
    failures = []
    for test in [stops_with_streams_open, ctrl_c_with_streams_open, refuses_duplicate_home,
                 exits_when_engine_crashes, gateway_follows_restarted_runtime]:
        try:
            test()
        except (AssertionError, subprocess.TimeoutExpired, OSError) as error:
            failures.append(test.__name__)
            print(f"FAIL {test.__name__}: {error}", flush=True)
    if failures:
        raise SystemExit(1)
