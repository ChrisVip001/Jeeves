#!/usr/bin/env python3
"""Owner chat → task → reviewed final copy → simulated Feishu receipt."""
import importlib.util
import json
import socket
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError

spec = importlib.util.spec_from_file_location("interaction", Path(__file__).with_name("smoke-interaction.py"))
interaction = importlib.util.module_from_spec(spec)
spec.loader.exec_module(interaction)
web = interaction.web


def until(read):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        value = read()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError("Timed out waiting for the fixture state")


def check(home, connector, posts, *, route):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    cfg = dict(app_id="cli_promotion_fixture", app_secret="env:JEEVES_PROMOTION_FIXTURE",
               base_url=connector, receive_id_type="chat_id", live=True,
               allow_recipients=["oc_first","oc_final"])
    Path(home,"connectors.json").write_text(json.dumps({"feishu":cfg}))
    provider, process = web.start(home,port,{"JEEVES_PROMOTION_FIXTURE":"fixture-only"})
    base=f"http://127.0.0.1:{port}"

    def ready():
        try:
            return web.request(base,"/api/session/main")
        except HTTPError as error:
            raise AssertionError(f"Dashboard returned {error.code}: {error.read()!r}") from error
        except OSError:
            if process.poll() is not None:
                raise AssertionError(Path(home, "server.log").read_text())
            return False

    def pending():
        rows=web.request(base,"/api/approval/pending?sessionId=jeeves-main")["approvals"]
        return next((row["request"] for row in rows if row["request"]["tool"]=="feishu_send"),None)

    def answer(verb, ask, uid, version=None):
        v=ask["version"] if version is None else version
        return interaction.interact(base,f"{verb} {ask['requestId']} v{v}",uid)

    def notices(source):
        return [row for row in web.request(base,"/api/session?sessionId=jeeves-main")["data"] if row.get("source")==source]

    try:
        until(ready)
        assignment="Prepare the open-source-promotion-fixture first edition"
        interaction.interact(base,assignment,"promotion-assignment")
        interaction.interact(base,assignment,"promotion-assignment")
        first=until(pending)
        rows=interaction.tasks(base)
        assert len(rows)==1
        task=rows[0]["id"]
        conversation = "/api/session?sessionId=" + rows[0]["executionSessionId"]
        assert web.request(base,conversation)["activeInputs"] == [assignment]
        copy1="First edition: "+"Synthetic open source announcement. "*10+"End of first copy."
        review=until(lambda:next((row["content"] for row in notices("task_decision") if first["requestId"] in row["content"]),None))
        assert all(value in review for value in [copy1,"Feishu","oc_first","cli_promotion_fixture",task])
        answer("approve",first,"wrong-version",first["version"]+1)
        assert not posts and pending()["requestId"]==first["requestId"]
        # A revision delivered to the existing task retires the outstanding approval.
        revision="open-source-promotion-fixture final edition"
        if route == "steer":
            web.request(base, "/api/turn/steer", {"sessionId":rows[0]["executionSessionId"],"message":revision})
        elif route == "reference":
            revision += f" #{task}"
            interaction.interact(base,revision,"promotion-revision")
        else:
            interaction.interact(base,revision,"promotion-revision",task)
        stale=answer("approve",first,"stale-approval")
        assert not any("Approved once" in event.get("reply","") for event in stale), "old copy remained approvable"
        current=until(lambda: (r if (r:=pending()) and r["requestId"]!=first["requestId"] else None))
        assert web.request(base,conversation)["activeInputs"] == [assignment, revision]
        copy2="Final edition: "+"Synthetic open source announcement. "*10+"End of revised copy."
        assert all(x in current["argsPreview"] for x in [copy2,"oc_final","cli_promotion_fixture","Feishu"])
        assert not posts
        answer("批准",current,"approve-final")
        answer("批准",current,"approve-final")
        answer("批准",current,"second-answer")
        row=interaction.settled(base,task)
        assert len(interaction.tasks(base))==1 and row["status"]=="done"
        assert posts==[("oc_final",copy2)], posts
        completions=until(lambda:notices("task_completion"))
        assert len(completions)==1 and task in completions[0]["content"],completions
        page=web.request(base,conversation)
        assert not page.get("activeInputs")
        assert web.fixture.PROMOTION_RECEIPT in completions[0]["content"], "Main chat lost the task result"
        messages=page["data"]
        assert [m["content"] for m in reversed(messages) if m["role"]=="user"] == [assignment,revision]
        assert any(m["role"]=="assistant" for m in messages)
    finally:
        process.terminate()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        provider.shutdown()
        provider.server_close()


def mock_channel(posts):
    class MockChannel(BaseHTTPRequestHandler):
        def log_message(self,*_): pass

        def reply(self, **values):
            body=json.dumps(dict(code=0,**values)).encode()
            self.send_response(200); self.send_header("Content-Length",str(len(body))); self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            packet=json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if self.path.endswith("/internal"):
                self.reply(tenant_access_token="local-fixture")
            else:
                posts.append((packet["receive_id"],json.loads(packet["content"])["text"]))
                self.reply(data=dict(message_id="om_synthetic"))

        def do_GET(self):
            self.reply(data=dict(items=[dict(message_id="om_synthetic")]))
    return ThreadingHTTPServer(("127.0.0.1",0),MockChannel)


def main():
    posts=[]
    with mock_channel(posts) as channel:
        threading.Thread(target=channel.serve_forever,daemon=True).start()
        try:
            for route in ("target", "steer", "reference"):
                posts.clear()
                with tempfile.TemporaryDirectory() as home:
                    check(home,f"http://127.0.0.1:{channel.server_port}",posts,route=route)
        finally:
            channel.shutdown()
    print("PASS promotion: one task, final copy, stale approval refused, one mock post and completion")


if __name__=="__main__":
    main()
