#!/usr/bin/env python3
# coding: utf-8
"""Use the Web composer and a test clock against one disposable Runtime database."""

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import hmac
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
from urllib import error, request
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
EVENT_REQUEST = "报销邮箱收到新邮件时，仅当发件人是财务组而且邮件有 PDF 附件，就在这里提醒我并附原邮件来源。"
EVENT_PAUSE = "停用刚才的报销邮件提醒。"
EVENT_MESSAGE = "收到一封待处理的报销邮件"
EVENT_SOURCE = "expense-inbox"
EVENT_TYPE = "mail.received"
SENDER = "finance@example.test"
OPEN, DONE = "Open fixture assignment", "Completed fixture assignment"


def digest_plan(cron, sources, window, layout):
    return dict(
        trigger=dict(kind="cron", expression=cron, timeZone=ZONE.key),
        action=dict(
            kind="digest",
            version=1,
            config=dict(
                sources=sources,
                options=dict(timeZone=ZONE.key, calendarWindow=window, layout=layout),
            ),
        ),
        delivery=dict(kind="chat"),
    )


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
            if EVENT_REQUEST in user:
                args = dict(
                    action="create",
                    id="expense-alert",
                    fields=dict(
                        purpose="报销邮件提醒",
                        plan=dict(
                            trigger=dict(
                                kind="event", source=EVENT_SOURCE, eventType=EVENT_TYPE
                            ),
                            action=dict(
                                kind="records.evaluate",
                                version=1,
                                config=dict(
                                    all=[
                                        dict(field="sender", op="in", value=[SENDER]),
                                        dict(
                                            field="attachmentTypes",
                                            op="contains",
                                            value="application/pdf",
                                        ),
                                    ],
                                    include=["subject", "messageRef"],
                                    message=EVENT_MESSAGE,
                                ),
                            ),
                            delivery=dict(kind="chat"),
                        ),
                    ),
                )
            elif EVENT_PAUSE in user:
                args = dict(action="pause", id="expense-alert", expectedRevision=1)
            elif ALTERNATE in user:
                args.update(
                    action="create",
                    id="daily-agenda",
                    fields={
                        "purpose": "当日日程",
                        "plan": digest_plan("0 8 * * *", ["calendar"], "today", "list"),
                    },
                )
            elif AMEND in user:
                args.update(
                    action="amend",
                    expectedRevision=1,
                    fields={"plan": digest_plan("0 15 * * 4", ["calendar"], "nextWeek", "paragraphs")},
                )
            elif PAUSE in user:
                args.update(action="pause", expectedRevision=2)
            else:
                assert REQUEST in user
                args.update(
                    action="create",
                    fields={
                        "purpose": "工作安排",
                        "plan": digest_plan(
                            "0 15 * * 5", ["calendar", "openTasks"], "nextWeek", "paragraphs"
                        ),
                    },
                )
            web.fixture.inline_delivery_fixture.calls(
                self, body["tools"], [("recurring_work", args)]
            )
        self.wfile.write(b"data: [DONE]\n\n")


def browser_action(home, workspace, cookie, action, message):
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


def event_work(home, managed):
    secret = "disposable-event-signing-key"
    (home / "webhooks.json").write_text(
        json.dumps(
            {
                "hooks": [
                    dict(
                        name=EVENT_SOURCE,
                        event_type=EVENT_TYPE,
                        signature=True,
                        secret=secret,
                        event_fields=dict(
                            sender="string",
                            attachmentTypes="stringArray",
                            subject="string",
                            messageRef="string",
                        ),
                    )
                ]
            }
        )
    )
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    web.fixture.Provider = Source
    provider, runtime = web.start(home, port)
    provider.codes = Queue()
    provider.outbound = []
    base = f"http://127.0.0.1:{port}"

    def ready():
        for _ in range(150):
            try:
                web.request(base, "/api/session/main")
                return
            except OSError:
                time.sleep(0.1)
        raise AssertionError("Runtime unavailable")

    def event(n, sender=SENDER, mime="application/pdf"):
        return dict(
            eventId=f"message-{n}",
            occurredAt=datetime.now(timezone.utc).isoformat(),
            cursor=f"offset-{n}",
            previousCursor=f"offset-{n - 1}" if n else None,
            fields=dict(
                sender=sender,
                attachmentTypes=[mime],
                subject=f"Fixture {n}",
                messageRef=f"mail://fixture/{n}",
            ),
        )

    def post(packet, valid=True):
        body = json.dumps(packet, sort_keys=True).encode()
        stamp = str(int(time.time()))
        signature = hmac.new(
            secret.encode(), stamp.encode() + b"." + body, hashlib.sha256
        ).hexdigest()
        req = request.Request(
            base + "/api/hook/" + EVENT_SOURCE,
            data=body,
            headers={
                "X-Jeeves-Timestamp": stamp,
                "X-Jeeves-Signature": signature if valid else "bogus",
            },
        )
        try:
            with request.urlopen(req, timeout=10) as resp:
                assert resp.status == 202
                return json.loads(resp.read())
        except error.HTTPError as exc:
            return exc.code

    try:
        ready()
        mail = (
            (f"http://127.0.0.1:{provider.server_port}/registration", provider.codes)
            if managed
            else None
        )
        with (
            gateway(base, home, mail_fixture=mail) as (_, workspace, cookie),
            sqlite3.connect(home / "state.db", isolation_level=None) as db,
        ):
            fixture = event(0)
            assert post(fixture, valid=False) == 401
            assert post(fixture) == {"admitted": 0, "duplicate": False}
            created = browser_action(
                home, workspace, cookie, "event-create", EVENT_REQUEST
            )
            assert "mail://fixture/0" in created["samples"][0]
            assert db.execute(
                "SELECT cursor FROM responsibility_event_cursors"
            ).fetchone() == ("offset-0",)
            assert db.execute(
                "SELECT count(*) FROM responsibility_event_positions"
            ).fetchone() == (1,)
            baseline = db.execute("SELECT count(*) FROM model_requests").fetchone()
            no_sender = event(1, sender="other@example.test")
            no_pdf = event(2, mime="text/plain")
            matched = event(3)
            assert post(matched) == 409
            for packet in [no_sender, no_pdf, matched]:
                assert post(packet) == {"admitted": 1, "duplicate": False}
            assert post(matched) == {"admitted": 0, "duplicate": True}
            conflict = {
                **matched,
                "fields": {**matched["fields"], "subject": "Changed"},
            }
            assert post(conflict) == 409

            def runs():
                return db.execute(
                    "SELECT coalesce(outcome,status) FROM responsibility_runs ORDER BY rowid"
                ).fetchall()

            for _ in range(150):
                if runs() == [("skipped",), ("skipped",), ("confirmed",)]:
                    break
                time.sleep(0.1)
            else:
                raise AssertionError(runs())
            assert (
                db.execute("SELECT count(*) FROM model_requests").fetchone() == baseline
            )
            runtime.terminate()
            runtime.wait(timeout=20)
            runtime = web.start_runtime(home, port)
            ready()
            assert post(matched) == {"admitted": 0, "duplicate": True}
            browser_action(home, workspace, cookie, "event-pause", EVENT_PAUSE)
            assert post(event(4)) == {"admitted": 0, "duplicate": False}
            deliveries = db.execute(
                "SELECT content FROM chat_log WHERE source LIKE 'responsibility:expense-alert:event:%'"
            ).fetchall()
            assert (
                len(deliveries) == 1
                and "mail://fixture/3" in deliveries[0][0]
                and "message-3" in deliveries[0][0]
            )
            assert (
                runs() == [("skipped",), ("skipped",), ("confirmed",)]
                and not provider.outbound
            )
            print(
                json.dumps(
                    dict(
                        case="mail-event",
                        mode="managed" if managed else "selfHosted",
                        id="expense-alert",
                        runs=runs(),
                        deliveries=len(deliveries),
                        executionModelRequests=0,
                        externalSends=0,
                    )
                )
            )
    finally:
        if runtime.poll() is None:
            runtime.terminate()
            runtime.wait(timeout=20)
        provider.shutdown()


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
    parser.add_argument("--case", choices=["both", "timer", "event"], default="both")
    args = parser.parse_args()
    for mode in ["selfHosted", "managed"] if args.mode == "both" else [args.mode]:
        with tempfile.TemporaryDirectory(prefix="jeeves-recurring-") as temp:
            if args.case != "event":
                run(Path(temp), mode == "managed")
        if args.case != "timer":
            with tempfile.TemporaryDirectory(prefix="jeeves-recurring-event-") as temp:
                event_work(Path(temp), mode == "managed")
