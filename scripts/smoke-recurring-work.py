#!/usr/bin/env python3
# coding: utf-8
"""Use the Web composer and a test clock against one disposable Runtime database."""

import argparse
from datetime import datetime, timedelta, timezone
import importlib.util
import json
import os
from pathlib import Path
from queue import Queue
import socket
import sqlite3
import subprocess
import tempfile
import time
from zoneinfo import ZoneInfo

from gateway_fixture import gateway

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("web", ROOT / "scripts/smoke-web.py")
web = importlib.util.module_from_spec(spec)
spec.loader.exec_module(web)
ZONE = ZoneInfo("Asia/Shanghai")
REQUEST = "每周五下午把下周会议和当前未完成任务整理成两段发到这个聊天，带来源。"
AMEND = "改到周四，其他时间不变，只保留会议。"
PAUSE = "暂停刚才这项周期委托。"
ALTERNATE = "再给我看一个不同的安排：每天列出今天的日历，先预览。"
OPEN, DONE = "Open fixture assignment", "Completed fixture assignment"


class Source(web.fixture.Provider):
    def do_GET(self):
        if self.path != "/fixture.ics":
            return self.send_error(404)
        self.send_response(200)
        self.end_headers()
        self.wfile.write(self.server.calendar)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/registration":
            self.server.codes.put(body["text"].split("\n\n")[2].strip())
            self.send_response(202)
            self.end_headers()
            return
        if self.path == "/notify":
            self.server.outbound.append(body)
            self.send_response(200)
            self.end_headers()
            return
        if not body.get("stream"):
            self.send_response(200)
            self.end_headers()
            self.wfile.write(
                json.dumps(
                    {
                        "choices": [
                            {
                                "message": {"content": '{"kind":"conversation"}'},
                                "finish_reason": "stop",
                            }
                        ]
                    }
                ).encode()
            )
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        latest = body["messages"][-1]
        if latest["role"] == "tool":
            self.chunk({"content": "请核对这张委托卡。"}, "stop")
        else:
            user = next(
                m["content"] for m in reversed(body["messages"]) if m["role"] == "user"
            )
            args = {"id": "planning-digest"}
            if ALTERNATE in user:
                args.update(
                    action="create",
                    id="daily-agenda",
                    fields={
                        "purpose": "当日日程",
                        "cron": "0 8 * * *",
                        "sources": ["calendar"],
                        "digest": {
                            "timeZone": ZONE.key,
                            "calendarWindow": "today",
                            "layout": "list",
                        },
                    },
                )
            elif AMEND in user:
                args.update(
                    action="amend",
                    expectedRevision=1,
                    fields={"cron": "0 15 * * 4", "sources": ["calendar"]},
                )
            elif PAUSE in user:
                args.update(action="pause", expectedRevision=2)
            else:
                assert REQUEST in user
                args.update(
                    action="create",
                    fields={
                        "purpose": "工作安排",
                        "cron": "0 15 * * 5",
                        "sources": ["calendar", "openTasks"],
                        "digest": {
                            "timeZone": ZONE.key,
                            "calendarWindow": "nextWeek",
                            "layout": "paragraphs",
                        },
                    },
                )
            web.fixture.inline_delivery_fixture.calls(
                self, body["tools"], [("recurring_work", args)]
            )
        self.wfile.write(b"data: [DONE]\n\n")


def run(home, managed):
    now = datetime.now(ZONE)
    friday = (now + timedelta(days=(4 - now.weekday()) % 7)).replace(
        hour=15, minute=0, second=0, microsecond=0
    )
    if friday <= now:
        friday += timedelta(weeks=1)
    thursday = friday + timedelta(days=6)

    def monday(at):
        return (at + timedelta(days=7 - at.weekday())).replace(
            hour=0, minute=0, second=0, microsecond=0
        )

    def event(at, label):
        start = at.astimezone(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        return [
            "BEGIN:VEVENT",
            "UID:" + label,
            "DTSTART:" + start,
            "SUMMARY:" + label,
            "END:VEVENT",
        ]

    entries = event(now, "today-only")
    for day in sorted({monday(t) for t in [now, friday, thursday]}):
        entries.extend(event(day, "meeting-" + day.date().isoformat()))
    entries.extend(
        event(
            min(monday(t) for t in [now, friday]) - timedelta(minutes=1),
            "outside-window",
        )
    )
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    web.fixture.Provider = Source
    provider, runtime = web.start(home, port)
    provider.codes = Queue()
    provider.outbound = []
    source = f"http://127.0.0.1:{provider.server_port}"
    (home / "notify.json").write_text(json.dumps({"slack": source + "/notify"}))
    provider.calendar = (
        "\r\n".join(
            [
                "BEGIN:VCALENDAR",
                "VERSION:2.0",
                "PRODID:-//Fixture//EN",
                *entries,
                "END:VCALENDAR",
                "",
            ]
        )
    ).encode()
    base = f"http://127.0.0.1:{port}"
    (home / "connectors.json").write_text(
        json.dumps(
            {
                "calendar": {
                    "url": f"http://127.0.0.1:{provider.server_port}/fixture.ics",
                    "timezone": ZONE.key,
                }
            }
        )
    )

    def ready():
        for _ in range(200):
            try:
                web.request(base, "/api/session/main")
                return
            except OSError:
                time.sleep(0.1)
        raise AssertionError("Runtime unavailable")

    def stop():
        runtime.terminate()
        runtime.wait(timeout=15)

    def clock(*times):
        nonlocal runtime
        stop()
        for at in times:
            (home / "fixture-clock").write_text(at.isoformat())
            subprocess.run(
                [
                    "cargo",
                    "test",
                    "-q",
                    "-p",
                    "jeeves-app-server",
                    "advance_web_fixture_clock",
                    "--",
                    "--ignored",
                ],
                cwd=ROOT,
                env=dict(os.environ, RECURRING_SMOKE_HOME=str(home)),
                check=True,
                stdout=subprocess.DEVNULL,
            )
        runtime = web.start_runtime(home, port, {"JEEVES_NOTIFY": "1"})
        ready()

    try:
        ready()
        mail = (source + "/registration", provider.codes) if managed else None
        with (
            gateway(base, home, mail_fixture=mail) as (_, workspace, cookie),
            sqlite3.connect(home / "state.db", isolation_level=None) as db,
        ):
            for title, status in [(OPEN, "pending"), (DONE, "done")]:
                db.execute(
                    "INSERT INTO agent_tasks(id,title,status) VALUES(?,?,?)",
                    (title.split()[0], title, status),
                )

            def chat(action, message):
                result = subprocess.run(
                    [
                        "node",
                        ROOT / "scripts/smoke-recurring-work.mjs",
                        workspace,
                        cookie,
                        action,
                        message,
                        home / "state.db",
                    ],
                    check=True,
                    text=True,
                    stdout=subprocess.PIPE,
                )
                return json.loads(result.stdout)

            def deliveries():
                return db.execute(
                    "SELECT source,content FROM chat_log WHERE session_id='jeeves-main' AND source LIKE 'responsibility:%' ORDER BY id"
                ).fetchall()

            def check_body(body, at, tasks):
                assert (
                    "meeting-" + monday(at).date().isoformat() in body
                    and "(calendar meeting-" in body
                ), body
                assert (
                    "today-only" not in body
                    and "outside-window" not in body
                    and DONE not in body
                )
                assert body.count("(calendar ") == 1, body
                assert (OPEN in body) == tasks and ("(task Open)" in body) == tasks
                assert len(body.split("\n\n")) == (2 if tasks else 1), body

            created = chat("create", REQUEST)
            for sample in created["samples"]:
                check_body(sample, now, True)
            ident = created["review"]["contract"]["id"]
            assert not deliveries()
            clock(friday, friday)
            first = deliveries()
            assert len(first) == 1
            check_body(first[0][1], friday, True)
            changed = chat("amend", AMEND)
            assert (
                changed["review"]["contract"]["id"] == ident
                and changed["review"]["expectedRevision"] == 1
            )
            clock(thursday, thursday + timedelta(days=1))
            assert len(deliveries()) == 2
            check_body(deliveries()[1][1], thursday, False)
            stopped = chat("pause", PAUSE)
            clock(thursday + timedelta(weeks=1))
            assert db.execute(
                "SELECT status,revision FROM responsibilities WHERE id=?", (ident,)
            ).fetchone() == ("paused", 3)
            runs = db.execute(
                "SELECT p.wake_key,w.responsibility_revision,p.status,w.state FROM responsibility_runs p JOIN wake_inbox w ON p.wake_key=w.source_key ORDER BY p.rowid"
            ).fetchall()
            assert (
                runs
                == [
                    (key, rev, "confirmed", "delivered")
                    for rev, (key, _) in enumerate(deliveries(), 1)
                ]
                and len(runs) == 2
            ), runs
            assert (
                not db.execute("SELECT 1 FROM receipts").fetchone()
                and not db.execute("SELECT 1 FROM pending_approvals").fetchone()
            )
            variant = chat("preview", ALTERNATE)
            assert (
                "today-only" in variant["samples"][0]
                and "(calendar meeting-" not in variant["samples"][0]
            )
            assert db.execute("SELECT count(*) FROM responsibilities").fetchone() == (
                1,
            )
            assert not provider.outbound, provider.outbound
            print(
                json.dumps(
                    dict(
                        mode="managed" if managed else "selfHosted",
                        provider="scripted",
                        id=ident,
                        runs=runs,
                        conversation=[
                            (x["review"]["request"], x["reply"])
                            for x in [created, changed, stopped]
                        ],
                        externalSends=0,
                    ),
                    ensure_ascii=False,
                )
            )
    finally:
        if runtime.poll() is None:
            stop()
        provider.shutdown()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--mode", choices=["both", "managed", "selfHosted"], default="both"
    )
    args = parser.parse_args()
    for mode in ["selfHosted", "managed"] if args.mode == "both" else [args.mode]:
        with tempfile.TemporaryDirectory(prefix="jeeves-recurring-") as temp:
            run(Path(temp), mode == "managed")
