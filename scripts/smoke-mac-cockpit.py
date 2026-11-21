#!/usr/bin/env python3
"""Operate Jeeves.app's own foreground WebKit window on macOS, using synthetic services."""
import importlib.util
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path
from gateway_fixture import EMAIL, PASSWORD, RUNTIME_TOKEN, gateway

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "promotion", Path(__file__).with_name("smoke-promotion.py")
)
promotion = importlib.util.module_from_spec(spec)
spec.loader.exec_module(promotion)
fixture = promotion.web.fixture
APP = ROOT / "desktop/src-tauri/target/release/bundle/macos/Jeeves.app"
BIN = APP / "Contents/MacOS"
OUTPUT = (
    ROOT / "jeeves-rs/target/acceptance/mac-cockpit" / time.strftime("%Y%m%d-%H%M%S")
)


def wait_for(read, description, timeout=32):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = read()
            if value:
                return value
        except (OSError, subprocess.CalledProcessError) as error:
            last = getattr(error, "stderr", None) or str(error)
        time.sleep(0.25)
    raise AssertionError(f"{description}: {last or 'not observed'}")


class Cockpit:
    def __init__(self, probe, home, port, env, bundle):
        self.bundle = bundle
        self.probe, self.port = probe, port
        self.log = (home / "desktop.log").open("a")
        self.process = subprocess.Popen(
            [bundle / "Contents/MacOS/jeeves-desktop"],
            env=env,
            cwd="/",
            stdout=self.log,
            stderr=subprocess.STDOUT,
        )
        self.listener = None

    def native(self, *args):
        result = subprocess.run(
            [self.probe, str(self.process.pid), str(self.bundle), str(self.port), *args],
            check=True,
            capture_output=True,
            text=True,
            timeout=8,
        )
        return json.loads(result.stdout)

    def ready(self):
        def listening():
            if self.process.poll() is not None:
                raise AssertionError(f"Desktop exited with {self.process.returncode}")
            children = subprocess.run(
                ["pgrep", "-P", str(self.process.pid)], capture_output=True, text=True
            )
            for pid in children.stdout.split():
                listener = subprocess.run(
                    [
                        "lsof",
                        "-a",
                        "-nP",
                        "-p",
                        pid,
                        f"-iTCP@127.0.0.1:{self.port}",
                        "-sTCP:LISTEN",
                    ],
                    capture_output=True,
                    text=True,
                )
                if listener.returncode == 0:
                    return int(pid)

        self.listener = wait_for(listening, "bundled runtime did not listen", 70)
        wait_for(lambda: self.native("activate"), "foreground activation")
        return wait_for(lambda: self.native("window"), "visible native WebKit fixture")

    def get(self, path):
        if self.listener is None or self.process.poll() is not None:
            raise RuntimeError("The verified fixture runtime is unavailable")
        with urllib.request.urlopen(
            urllib.request.Request(f"http://127.0.0.1:{self.port}" + path,
                headers={"Authorization": "Bearer " + RUNTIME_TOKEN}), timeout=3
        ) as response:
            return json.load(response)

    def sign_in(self):
        outcome = wait_for(lambda: "login" if self.control("account-email") else
                           "workspace" if self.control("mode-assistant") else None, "login or workspace")
        if outcome == "login":
            self.native("fill", "account-email", EMAIL)
            self.native("fixture-password", "account-password", PASSWORD)
            self.native("press", "account-submit")
            wait_for(lambda: self.control("mode-assistant"), "signed-in workspace")

    def view(self):
        return self.native("inspect")

    def control(self, name):
        return any(n["id"] == name for n in self.view())

    def see(self, text):
        return any(text in n["label"] for n in self.view())

    def submit(self, field, button, words):
        wait_for(lambda: self.control(field), field)
        self.native("fill", field, words)
        wait_for(
            lambda: any(n["id"] == field and n["value"] == words for n in self.view()),
            "typed fixture input",
        )
        self.native("press", button)
        print("Submitted using", field, flush=True)

    def capture(self, name):
        window = self.native("window")
        if window["screenCapture"]:
            path = OUTPUT / (name + ".png")
            subprocess.run(
                ["screencapture", "-x", "-o", "-l", str(window["window"]), str(path)],
                check=True,
            )
            if path.stat().st_size < 1000:
                raise AssertionError("Window capture is empty")
            window["screenshot"] = path.name
        return window

    def close(self):
        try:
            if self.process.poll() is None:
                self.native("quit")
                self.process.wait(timeout=45)
        finally:
            self.log.close()
            if self.process.poll() is None:
                self.process.terminate()
                self.process.wait(timeout=10)
            if self.listener:
                # Wait for this shell's own bundled runtime to release its listener.
                wait_for(
                    lambda: subprocess.run(
                        [
                            "lsof",
                            "-a",
                            "-p",
                            str(self.listener),
                            f"-iTCP:{self.port}",
                            "-sTCP:LISTEN",
                        ],
                        capture_output=True,
                    ).returncode
                    != 0,
                    "fixture runtime shutdown",
                    12,
                )


def exercise(home, port, env, probe, posts, bundle):
    app = Cockpit(probe, home, port, env, bundle)
    evidence = {"initialWorkingDirectory": "/", "initialHomeOverride": "JEEVES_HOME" in env}
    try:
        evidence["launch"] = app.ready()
        print("Visible native window", evidence["launch"], flush=True)
        app.sign_in()
        wait_for(lambda: app.control("mode-assistant"), "assistant mode switch")
        app.native("press", "mode-assistant")
        app.submit("input", "send", "Hello from the native Mac fixture")
        wait_for(lambda: app.see("桌面连接测试完成"), "ordinary chat response")
        evidence["tasksAfterOrdinaryChat"] = len(app.get("/api/tasks")["tasks"])
        assert evidence["tasksAfterOrdinaryChat"] == 0
        app.submit(
            "input", "send", "Prepare the open-source-promotion-fixture first edition"
        )
        tasks = lambda: app.get("/api/tasks")["tasks"]
        task = wait_for(lambda: next(iter(tasks()), None), "assigned task")
        task_id = task["id"]
        evidence["taskId"] = task_id

        def decision(previous=""):
            requests = app.get("/api/approval/pending?sessionId=jeeves-main")[
                "approvals"
            ]
            return next(
                (
                    r["request"]
                    for r in requests
                    if r["request"]["tool"] == "feishu_send"
                    and r["request"]["requestId"] != previous
                ),
                None,
            )

        first = wait_for(decision, "first publication review")
        assert app.control("input") and not app.control("cowork-input")

        def open_review(request):
            button = f"allow-{request['requestId']}-{request['version']}"
            wait_for(lambda: app.control("chat-review-requests"), "live decision in Chat")
            if not app.control(button):
                app.native("press", "chat-review-requests")
            wait_for(lambda: app.control(button), "reviewed request and version")
            return button

        open_review(first)
        evidence["assistant"] = app.capture("01-assistant")
        app.native("press", "mode-cowork")
        target = "cowork-task-" + task_id
        wait_for(lambda: app.control(target), "same task in Cowork")
        app.native("press", target)
        wait_for(lambda: app.control("cowork-input"), "task conversation")
        app.native("press", "work-history-toggle")
        wait_for(
            lambda: app.see("Prepare the open-source-promotion-fixture first edition"),
            "visible active instruction",
        )
        evidence["cowork"] = app.capture("02-cowork")
        app.submit(
            "cowork-input",
            "cowork-send",
            "Update the open-source-promotion-fixture second edition",
        )
        revised = wait_for(
            lambda: decision(first["requestId"]), "Cowork follow-up and fresh approval"
        )
        app.native("press", "mode-assistant")
        app.submit(
            "input",
            "send",
            f"Update the open-source-promotion-fixture final edition #{task_id}",
        )
        final = wait_for(
            lambda: decision(revised["requestId"]), "routed main chat follow-up"
        )
        assert (
            "End of revised copy." in final["argsPreview"]
            and "oc_final" in final["argsPreview"]
        )
        assert len(tasks()) == 1 and not posts
        allow_final = open_review(final)
        final_copy = (
            "Final edition: "
            + "Synthetic open source announcement. " * 10
            + "End of revised copy."
        )
        review_fields = [
            "Feishu",
            "cli_native_fixture",
            "oc_final",
            final_copy,
        ]
        assert any(
            all(field in node["label"] for field in review_fields)
            for node in app.view()
        ), "Incomplete publication review in the native main chat"
        evidence["decision"] = app.capture("03-final-review")
        app.native("press", allow_final)

        def completed():
            return next(
                (t for t in tasks() if t["id"] == task_id and t["status"] == "done"),
                None,
            )

        completed_task = wait_for(completed, "original task completed")
        execution = completed_task["executionSessionId"]
        main = lambda: app.get("/api/session?sessionId=jeeves-main")["data"]

        def summary():
            rows = [
                r
                for r in main()
                if r.get("source") == "task_completion" and task_id in r["content"]
            ]
            assert len(rows) <= 1
            return rows[0] if rows else None

        notice = wait_for(summary, "completion in original chat")
        assert fixture.PROMOTION_RECEIPT in notice["content"]
        wait_for(
            lambda: app.see(completed_task["title"])
            and app.see(notice["content"].splitlines()[-1]),
            "visible completion",
        )
        evidence["completion"] = app.capture("04-completion")
        app.close()
        app = Cockpit(probe, home, port, dict(env, JEEVES_HOME=str(home)), bundle)
        evidence["reopen"] = app.ready()
        app.sign_in()
        assert evidence["reopen"]["pid"] != evidence["launch"]["pid"]
        app.native("press", "mode-assistant")
        wait_for(
            lambda: app.see(notice["content"].splitlines()[-1]),
            "completion after app reopen",
        )
        assert summary()["id"] == notice["id"] and [t["id"] for t in tasks()] == [
            task_id
        ]
        app.native("press", "mode-cowork")
        wait_for(lambda: app.control(target), "restored task")
        app.native("press", target)
        wait_for(
            lambda: app.see(fixture.PROMOTION_RECEIPT),
            "confirmed simulated publication result after reopen",
        )
        assert tasks()[0]["executionSessionId"] == execution
        assert (
            len(posts) == 1 and posts[0][0] == "oc_final" and posts[0][1] == final_copy
        )
        app.native("press", "work-history-toggle")
        for wording in ("first edition", "second edition", "final edition"):
            assert app.see(wording), f"Missing {wording} in reopened task"
        evidence["restoredConversation"] = app.capture("05-reopened-cowork")
        evidence.update(
            executionSessionId=execution,
            completionMessageId=notice["id"],
            completionNoticesAfterReopen=sum(
                row.get("source") == "task_completion" for row in main()
            ),
            tasksAfterReopen=len(tasks()),
            syntheticPosts=len(posts),
            approvals=[first["requestId"], revised["requestId"], final["requestId"]],
        )
        return evidence
    except Exception:
        try:
            (OUTPUT / "failure-state.json").write_text(
                json.dumps(
                    {
                        "tasks": app.get("/api/tasks"),
                        "messages": app.get("/api/session?sessionId=jeeves-main"),
                        "pending": app.get(
                            "/api/approval/pending?sessionId=jeeves-main"
                        ),
                    },
                    ensure_ascii=False,
                    indent=2,
                )
            )
            (OUTPUT / "failure.json").write_text(
                json.dumps(app.view(), ensure_ascii=False, indent=2)
            )
        except Exception as error:
            print(f"Fixture diagnostics unavailable: {error}", file=sys.stderr)
        raise
    finally:
        app.close()


def main():
    if sys.platform != "darwin" or not (BIN / "jeeves").is_file():
        raise SystemExit("Build Jeeves.app on macOS with just desktop-build first")
    OUTPUT.mkdir(parents=True, exist_ok=True)
    posts = []
    with tempfile.TemporaryDirectory(
        prefix="jeeves-native-cockpit-"
    ) as tmp, promotion.mock_channel(posts) as channel, fixture.ThreadingHTTPServer(
        ("127.0.0.1", 0), fixture.Provider
    ) as provider:
        # Stage the release bundle in a disposable checkout. Finder's "/" cwd must
        # select this checkout's existing data without any shell environment override.
        checkout = Path(tmp).resolve()
        home = checkout / ".jeeves"
        home.mkdir()
        user_home = checkout / "user-home"
        user_home.mkdir()
        bundle = checkout / APP.relative_to(ROOT)
        bundle.parent.mkdir(parents=True)
        subprocess.run(["/usr/bin/ditto", APP, bundle], check=True)
        for server in [channel, provider]:
            threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            probe = home / "CockpitWindow"
            subprocess.run(
                [
                    "xcrun",
                    "swiftc",
                    str(ROOT / "desktop/src-tauri/window/Window.swift"),
                    str(ROOT / "desktop/tests/CockpitWindow.swift"),
                    "-o",
                    str(probe),
                ],
                check=True,
            )
            base = {
                key: os.environ[key]
                for key in ("PATH", "TMPDIR", "LANG", "USER")
                if key in os.environ
            }
            base["HOME"] = str(user_home)
            rpc = fixture.Rpc(BIN / "jeeves", home, cwd=home, base_env=base)
            try:
                rpc.call(
                    "model/save",
                    dict(
                        id="native-fixture",
                        provider="ollama",
                        model="fixture",
                        smallModel="fixture",
                        baseUrl=f"http://127.0.0.1:{provider.server_port}/v1",
                        makeActive=True,
                    ),
                )
                rpc.call(
                    "config/write",
                    {"patch": {"consolidateEvery": 1000, "graphWorkflows": False}},
                )
            finally:
                rpc.close()
            config = dict(
                app_id="cli_native_fixture",
                app_secret="env:JEEVES_PROMOTION_FIXTURE",
                base_url=f"http://127.0.0.1:{channel.server_port}",
                receive_id_type="chat_id",
                live=True,
                allow_recipients=["oc_first", "oc_final"],
            )
            (home / "connectors.json").write_text(json.dumps({"feishu": config}))
            with socket.socket() as reservation:
                reservation.bind(("127.0.0.1", 0))
                port = reservation.getsockname()[1]
            env = dict(
                base,
                JEEVES_OPS_HOST="127.0.0.1",
                JEEVES_PORT=str(port),
                JEEVES_PROVIDER="ollama",
                JEEVES_WRITE="1",
                JEEVES_SEARCH_DDG="0",
                JEEVES_PROMOTION_FIXTURE="synthetic",
            )
            runtime = f"http://127.0.0.1:{port}"
            with gateway(runtime, home) as (origin, _, _):
                (home / "desktop-gateway").write_text(f"{origin}\n{runtime}\n")
                (home / "runtime-token").write_text(RUNTIME_TOKEN)
                evidence = exercise(home, port, env, probe, posts, bundle)
            (OUTPUT / "report.json").write_text(
                json.dumps(evidence, ensure_ascii=False, indent=2)
            )
            print(
                f"PASS native Mac cockpit, restarted app and runtime; evidence at {OUTPUT}"
            )
        finally:
            channel.shutdown()
            provider.shutdown()


if __name__ == "__main__":
    main()
