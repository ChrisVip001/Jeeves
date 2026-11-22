"""Exercise third-phase delivery contracts against a real local Runtime and controlled provider wires."""

import asyncio
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "recovery", Path(__file__).with_name("smoke-recovery.py")
)
recovery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recovery)
BINARY = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "debug/jeeves"
REPORT = "Report of the verified sample."
SOURCE = "https://example.test/original"


def use(key, tool, **args):
    return {"type": "tool_use", "id": key, "name": tool, "input": args}


def write(key, path, content):
    return use(key, "write_file", path=path, content=content)


def text(value):
    return {"type": "text", "text": value}


class Fixture:
    def __init__(self, root, case, wire):
        self.case, self.root, self.wire = case, root, wire
        self.release = threading.Event()
        self.context_inputs = []
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *unused):
                pass

            def do_POST(self):
                request = json.loads(
                    self.rfile.read(int(self.headers["Content-Length"]))
                )
                blocks = owner.answer(request)
                reason = "tool_use" if blocks[0]["type"] == "tool_use" else "end_turn"
                if wire == "anthropic":
                    usage = dict(
                        input_tokens=3,
                        cache_creation_input_tokens=4,
                        cache_read_input_tokens=3,
                        output_tokens=3,
                    )
                    if request.get("stream"):
                        events = [("message_start", {"message": {"usage": usage}})]
                        for i, block in enumerate(blocks):
                            initial = {
                                **block,
                                **(
                                    {"input": {}}
                                    if reason == "tool_use"
                                    else {"text": ""}
                                ),
                            }
                            delta = (
                                {
                                    "type": "input_json_delta",
                                    "partial_json": json.dumps(block["input"]),
                                }
                                if reason == "tool_use"
                                else {"type": "text_delta", "text": block["text"]}
                            )
                            events.extend(
                                [
                                    (
                                        "content_block_start",
                                        {"index": i, "content_block": initial},
                                    ),
                                    (
                                        "content_block_delta",
                                        {"index": i, "delta": delta},
                                    ),
                                    ("content_block_stop", {"index": i}),
                                ]
                            )
                        events.extend(
                            [
                                (
                                    "message_delta",
                                    {
                                        "delta": {"stop_reason": reason},
                                        "usage": {"output_tokens": 3},
                                    },
                                ),
                                ("message_stop", {}),
                            ]
                        )
                        payload = "".join(
                            f"event: {event}\ndata: {json.dumps(dict(data, type=event))}\n\n"
                            for event, data in events
                        )
                    else:
                        payload = json.dumps(
                            dict(content=blocks, stop_reason=reason, usage=usage)
                        )
                else:
                    usage = dict(
                        prompt_tokens=10,
                        completion_tokens=3,
                        prompt_tokens_details={"cached_tokens": 3},
                    )
                    delta = {"role": "assistant"}
                    if reason == "tool_use":
                        delta["tool_calls"] = [
                            dict(
                                index=i,
                                id=b["id"],
                                type="function",
                                function=dict(
                                    name=b["name"], arguments=json.dumps(b["input"])
                                ),
                            )
                            for i, b in enumerate(blocks)
                        ]
                    else:
                        delta["content"] = blocks[0]["text"]
                    finish = "tool_calls" if reason == "tool_use" else "stop"
                    if request.get("stream"):
                        chunk = dict(
                            choices=[dict(index=0, delta=delta, finish_reason=finish)],
                            usage=usage,
                        )
                        payload = "data: " + json.dumps(chunk) + "\n\ndata: [DONE]\n\n"
                    else:
                        payload = json.dumps(
                            dict(
                                choices=[dict(message=delta, finish_reason=finish)],
                                usage=usage,
                            )
                        )
                try:
                    self.send_response(200)
                    self.send_header(
                        "Content-Type",
                        "text/event-stream"
                        if request.get("stream")
                        else "application/json",
                    )
                    self.end_headers()
                    self.wfile.write(payload.encode())
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self.http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.http.serve_forever, daemon=True).start()

    def answer(self, request):
        if not request.get("tools"):
            return [text('{"retrieve":false}')]
        messages = request.get("messages", [])
        ids = {m.get("tool_call_id") for m in messages}
        ids.update(
            b.get("tool_use_id")
            for m in messages
            if isinstance(m.get("content"), list)
            for b in m["content"]
        )
        if self.case == "context":
            self.context_inputs.append(request)
            index = len(self.context_inputs)
            if index == 1:
                return [use("source", "testfeishu_quote", url=SOURCE)]
            if index == 3:
                self.pause()
            if index < 8:
                return [use(f"read-{index}", "read_file", path=f"notes-{index}.txt")]
            return [text("Reviewed the original source with the revised constraint.")]
        if self.case == "criteria":
            if "correct-json" in ids:
                return [text("Ready for verification.")]
            if "Acceptance did not pass" in json.dumps(messages):
                return [
                    write("correct-json", "answer.json", '{"answer":42}'),
                    write(
                        "correct-citation",
                        "claims.json",
                        json.dumps(claims("Sample was 42.")),
                    ),
                    write("correct-test", "result.txt", "passed"),
                ]
            return [text("Please check all three deliverables.")]
        if "report" not in ids:
            return [write("report", "report.md", REPORT)]
        if "sent" not in ids and self.case != "budget":
            if self.case == "before":
                self.pause()
            send = use("sent", "testfeishu_send", value=REPORT)
            if self.case == "lost":
                return [send, write("archive", "archive.md", REPORT)]
            return [send]
        if "archive" not in ids:
            if self.case in ("settled", "budget"):
                self.pause()
            return [write("archive", "archive.md", REPORT)]
        return [text("Delivery verified.")]

    def pause(self):
        if not self.release.is_set():
            (self.root / "pause").touch()
            self.release.wait(60)

    def close(self):
        self.release.set()
        self.http.shutdown()
        self.http.server_close()


def claims(quote):
    return [
        {
            "claimId": "c1",
            "text": "Sample was 42.",
            "sourceRefs": [SOURCE],
            "quote": quote,
        }
    ]


async def approvals(app):
    while True:
        ask = await app.approvals.get()
        assert ask["tool"] in (
            "write_file",
            "testfeishu_send",
            "testfeishu_quote",
            "run_command",
        ), ask
        if ask["tool"] == "testfeishu_send":
            assert json.loads(ask["argsPreview"]) == {"value": REPORT}
        result = await app.request(
            "approval/respond",
            dict(
                turnId=ask["turnId"],
                requestId=ask["requestId"],
                version=ask["version"],
                approved=True,
                remember="once",
            ),
        )
        assert result["accepted"]


async def scenario(case, wire="anthropic"):
    with tempfile.TemporaryDirectory(prefix="jeeves-contract-") as folder:
        root = Path(folder)
        home, work = root / "home", root / "work"
        home.mkdir()
        work.mkdir()
        fixture = Fixture(root, case, wire)
        program = recovery.MCP_PROGRAM.replace(
            "'name':'commit'", "'name':'send'"
        ).replace("mode=='after_effect'", "mode=='lost'")
        if case in ("before", "lost", "settled"):
            program = program.replace(
                "  append('requests')",
                """  with open(root/'received.jsonl','a') as f:
   print(json.dumps(m['params']['arguments']),file=f,flush=True); os.fsync(f.fileno())
  append('requests')""",
            )
        if case == "context":
            program = program.replace("'name':'send'", "'name':'quote'").replace(
                "'value'", "'url'"
            )
        (root / "mcp.py").write_text(program)
        (home / "mcp.json").write_text(
            json.dumps(
                {
                    "servers": [
                        dict(
                            name="testfeishu",
                            command=sys.executable,
                            args=[str(root / "mcp.py"), str(root), case],
                        )
                    ]
                }
            )
        )
        (home / "models.json").write_text(
            json.dumps(
                {
                    "active": "fixture",
                    "profiles": [
                        dict(
                            id="fixture",
                            provider="ollama" if wire == "openai" else wire,
                            model="fixture",
                            small_model="fixture",
                            base_url=f"http://127.0.0.1:{fixture.http.server_port}",
                            key_source="env:JEEVES_RECOVERY_TEST_KEY",
                        )
                    ],
                }
            )
        )
        criteria = [dict(kind="fileExists", path="archive.md")]
        if case == "context":
            criteria = []
            for i in range(2, 8):
                (work / f"notes-{i}.txt").write_text(
                    ("measurement series " + str(i) + "; ") * 480
                )
        if case == "criteria":
            (work / "sources").mkdir()
            (work / "sources/s.txt").write_text("Sample was 42.")
            (work / "sources/index.jsonl").write_text(
                json.dumps(
                    dict(
                        url=SOURCE,
                        path="sources/s.txt",
                        sha256=hashlib.sha256(
                            (work / "sources/s.txt").read_bytes()
                        ).hexdigest(),
                        fetchedAt="fixture",
                    )
                )
            )
            (work / "answer.json").write_text('{"answer":0}')
            (work / "claims.json").write_text(json.dumps(claims("Sample was 99.")))
            (work / "result.txt").write_text("failed")
            (work / "test.sh").write_text("grep -qx passed result.txt\n")
            criteria = [
                dict(
                    kind="jsonEquals",
                    path="answer.json",
                    pointer="/answer",
                    expectedJson="42",
                ),
                dict(kind="sourceSupports", path="claims.json"),
                dict(kind="commandSucceeds", argv=["sh", "test.sh"]),
            ]
        app = None
        approver = None

        async def boot():
            nonlocal app, approver
            app = await recovery.App.start(
                BINARY,
                home,
                work,
                approval="on-request",
                coding=True,
                extra_env={"JEEVES_CONTEXT_WINDOW": "8192"}
                if case == "context"
                else None,
            )
            approver = asyncio.create_task(approvals(app))

        async def stop():
            if approver:
                approver.cancel()
                await asyncio.gather(approver, return_exceptions=True)
            if app:
                await app.kill()

        try:
            await boot()
            params = dict(
                sessionId="jeeves-main",
                title="Delivery contract",
                cwd=str(work),
                maxRounds=3,
                acceptance=criteria,
            )
            if case == "budget":
                params["budget"] = dict(tokens=26)
            task = (await app.request("task/create", params))["task"]["id"]
            await app.request(
                "task/start",
                dict(
                    taskId=task,
                    message="Check the agreed deliverables."
                    if case == "criteria"
                    else "Write report, send to Feishu, then archive.",
                ),
            )
            with sqlite3.connect(home / "state.db", isolation_level=None) as db:
                if case == "context":
                    await recovery.wait_for(
                        lambda: (root / "pause").exists(), "amendment boundary"
                    )
                    change = (
                        "Use the revised ceiling of 17 and keep the original source."
                    )
                    await app.request(
                        "interaction/start",
                        dict(
                            sessionId="jeeves-main",
                            messageId="human-revision",
                            message=change,
                            taskId=task,
                        ),
                    )
                    fixture.release.set()
                    await recovery.status(app, task, "done")
                    inputs = [
                        json.dumps(r["messages"]) for r in fixture.context_inputs[3:]
                    ]
                    compact = [
                        request for request in inputs if "Context compacted." in request
                    ]
                    assert len(compact) >= 2 and all(
                        change in request and SOURCE in request for request in compact
                    ), [len(compact), inputs[-1][:2200]]
                    assert db.execute(
                        "SELECT instruction FROM task_amendments WHERE task_id=?",
                        (task,),
                    ).fetchone() == (change,)
                    print(
                        "PASS original source and revised ceiling survived",
                        len(compact),
                        "compacted model requests",
                        flush=True,
                    )
                    return
                if case == "criteria":
                    await recovery.status(app, task, "done")
                    rows = db.execute(
                        "SELECT checks_json FROM task_verifications WHERE task_id=? ORDER BY rowid",
                        (task,),
                    ).fetchall()
                    checks = [[c["passed"] for c in json.loads(row[0])] for row in rows]
                    assert checks == [[False] * 3, [True] * 3], checks
                    assert db.execute(
                        "SELECT COUNT(*) FROM agent_tasks"
                    ).fetchone() == (1,)
                    print(
                        "PASS JSON, unsupported quote and failing test remained open; all three remeasured in the same task",
                        flush=True,
                    )
                    return
                marker = "effect-ready" if case == "lost" else "pause"
                await recovery.wait_for(
                    lambda: (root / marker).exists(), f"{case} boundary"
                )
                assert (work / "report.md").read_text() == REPORT
                assert not (work / "archive.md").exists()
                operations = dict(
                    db.execute(
                        "SELECT tool_use_id,status FROM tool_operations WHERE task_id=?",
                        (task,),
                    )
                )
                assert operations["report"] == "succeeded"
                if case == "lost":
                    assert (
                        operations["sent"] == "intent"
                        and operations["archive"] == "not_executed"
                    ), operations
                if case == "settled":
                    assert operations["sent"] == "succeeded"
                if case == "before":
                    assert "sent" not in operations and not (root / "requests").exists()
                await stop()
                fixture.release.set()
                await boot()
                if case == "lost":
                    await recovery.status(app, task, "needs_attention")
                    published = await app.request("task/artifacts", dict(taskId=task))
                    assert any(
                        a["path"].endswith("/report.md")
                        and a.get("writtenText") == REPORT
                        for a in published["artifacts"]
                    ), published
                    ops = await app.request("task/operations", dict(taskId=task))
                    pending = [r for r in ops["operations"] if r["status"] == "intent"]
                    assert len(pending) == 1 and pending[0]["toolUseId"] == "sent"
                    assert (
                        recovery.count(root / "commits") == 1
                        and (work / "report.md").read_text() == REPORT
                    )
                    await app.request(
                        "task/reconcile",
                        dict(
                            taskId=task,
                            operationId=pending[0]["id"],
                            checkpointDigest=ops["checkpointDigest"],
                            resolution="reconciled_success",
                            evidence="Verified fixture Feishu receiver: one delivered message.",
                        ),
                    )
                    updated = await app.request("task/operations", dict(taskId=task))
                    assert {
                        op["toolUseId"]: op["status"] for op in updated["operations"]
                    } == {
                        "report": "succeeded",
                        "sent": "reconciled_success",
                        "archive": "not_executed",
                    }
                    assert not (work / "archive.md").exists()
                elif case == "budget":
                    state = await recovery.status(app, task, "needs_attention")
                    assert state["spent"]["tokens"] == 26, state
                    rows = db.execute(
                        "SELECT input_tokens,output_tokens,cache_read,cache_creation FROM model_requests WHERE termination IN ('tool_use','end_turn') AND run_id IS NOT NULL ORDER BY id"
                    ).fetchall()
                    assert db.execute(
                        "SELECT COUNT(*) FROM model_requests WHERE termination='interrupted' AND run_id IS NOT NULL AND input_tokens IS NULL"
                    ).fetchone() == (1,)
                    assert rows == [(10, 3, 3, 4 if wire == "anthropic" else 0)] * 2, (
                        rows
                    )
                    assert not (work / "archive.md").exists()
                else:
                    await recovery.status(app, task, "done")
                    assert (
                        work / "archive.md"
                    ).read_text() == REPORT and recovery.count(root / "commits") == 1
                if case in ("before", "lost", "settled"):
                    assert [
                        json.loads(line)
                        for line in (root / "received.jsonl").read_text().splitlines()
                    ] == [{"value": REPORT}]
                print("PASS", case, wire, flush=True)
        except BaseException:
            if (home / "process.log").exists():
                print((home / "process.log").read_text()[-2200:], file=sys.stderr)
            raise
        finally:
            await stop()
            fixture.close()


async def main():
    for case in ("before", "lost", "settled", "budget", "criteria", "context"):
        await scenario(case)
    await scenario("budget", "openai")


if __name__ == "__main__":
    asyncio.run(main())
