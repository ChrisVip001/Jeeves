#!/usr/bin/env python3
"""Exercise a resident Feishu research duty with temporary SQLite, JSON-RPC and loopback HTTP."""
import asyncio
from collections import Counter
from datetime import datetime, timedelta, timezone
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import sqlite3
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('recovery', ROOT / 'scripts/smoke-recovery.py')
recovery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recovery)
URL = 'https://evidence.example.test/weekly'
QUOTE = 'The fixture completed 12 reviews.'
CASES = {'confirmed': 'confirmed', 'denied': 'denied', 'wrong-id': 'review',
         'lost-post': 'review', 'missing': 'review', 'mismatch': 'review',
         'budget': 'review', 'paused': 'review', 'crash': 'review'}


def evidence(path, mode):
    path.mkdir()
    sources = path / 'sources'
    sources.mkdir()
    body = 'Weekly evidence. ' + QUOTE
    digest = hashlib.sha256(body.encode()).hexdigest()
    relative = f'sources/{digest[:16]}.txt'
    (path / relative).write_text(body)
    record = dict(url=URL, path=relative, sha256=digest,
                  fetchedAt=(datetime.now(timezone.utc) - timedelta(days=1)).isoformat())
    (sources / 'index.jsonl').write_text(json.dumps(record))
    quote = 'The fixture completed 99 reviews.' if mode == 'mismatch' else QUOTE
    claims = [dict(claimId='result', text=quote, quote=quote, sourceRefs=[URL])]
    if mode != 'missing':
        (path / 'claims.json').write_text(json.dumps(claims))


async def run(root, base, sends, crash_received):
    binary = ROOT / 'target/debug/jeeves'
    home = root / 'home'
    home.mkdir()
    config = dict(app_id='cli_fixture', app_secret='env:JEEVES_RECOVERY_TEST_KEY',
                  base_url=base, receive_id_type='chat_id', live=True,
                  allow_recipients=['oc_' + name for name in CASES])
    (home / 'connectors.json').write_text(json.dumps({'feishu': config}))
    app = await recovery.App.start(binary, home, root, approval='on-request')
    answered = set()
    try:
        for name in CASES:
            path = root / name
            evidence(path, name)
            task = await app.request('task/create', dict(title=name, cwd=str(path)))
            await app.request('responsibility/register', dict(
                id=name, purpose='Summarize the weekly research', sources=[URL],
                cron='* * * * *', expiresAt='2099-01-01T00:00:00Z',
                budget=dict(tools=1 if name == 'budget' else 2),
                weeklyReport=dict(taskId=task['task']['id'], recipient='oc_' + name)))
        await app.kill()
        app = await recovery.App.start(binary, home, root, approval='on-request')
        first_keys = {}
        finished = {}
        async def observe():
            nonlocal app
            while len(finished) < len(CASES):
                for name, expected in CASES.items():
                    if name in finished:
                        continue
                    rows = (await app.request('responsibility/reports', {'id': name}))['reports']
                    if not rows:
                        continue
                    key = first_keys.setdefault(name, rows[-1]['wakeKey'])
                    row = next(r for r in rows if r['wakeKey'] == key)
                    if name == 'crash' and crash_received.is_set() and row['status'] == 'sending':
                        await app.kill()
                        app = await recovery.App.start(binary, home, root, approval='on-request')
                        continue
                    request = row['approval']
                    if request and key not in answered:
                        assert request['turnId'] == key and request['tool'] == 'feishu_send'
                        assert hashlib.sha256(row['body'].encode()).hexdigest() in request['argsPreview']
                        assert 'chat_id:oc_' + name in request['argsPreview']
                        if name == 'paused':
                            await app.request('responsibility/setPaused', {'id': name, 'paused': True})
                        result = await app.request('approval/respond', dict(turnId=key,
                            requestId=request['requestId'], approved=name != 'denied',
                            remember='once', version=request['version']))
                        assert result['accepted']
                        answered.add(key)
                    if row['status'] in ('review','confirmed','denied','draft'):
                        assert row['status'] == expected, (name, row)
                        if name not in ('missing','mismatch'):
                            assert QUOTE in row['body'] and URL in row['body']
                        else:
                            assert row['body'] is None
                        assert row['toolsUsed'] == (2 if name in ('confirmed','wrong-id','lost-post','crash') else 1)
                        await app.request('responsibility/setPaused', {'id': name, 'paused': True})
                        finished[name] = row
                await asyncio.sleep(.05)
        await asyncio.wait_for(observe(), 90)
        await app.kill()
        config['live'] = False
        (home / 'connectors.json').write_text(json.dumps({'feishu': config}))
        app = await recovery.App.start(binary, home, root)
        task = await app.request('task/create', dict(title='Draft only', cwd=str(root / 'confirmed')))
        await app.request('responsibility/register', dict(id='draft', purpose='Local weekly draft',
            sources=[URL], cron='* * * * *', expiresAt='2099-01-01T00:00:00Z', budget=dict(tools=1),
            weeklyReport=dict(taskId=task['task']['id'],recipient='oc_confirmed')))
        await app.kill()
        app = await recovery.App.start(binary, home, root)
        async def draft():
            while True:
                rows = (await app.request('responsibility/reports', {'id':'draft'}))['reports']
                if rows and rows[0]['status'] == 'draft':
                    assert rows[0]['approval'] is None and QUOTE in rows[0]['body']
                    return
                await asyncio.sleep(.1)
        await asyncio.wait_for(draft(), 40)
        await app.kill()
        with sqlite3.connect(home / 'state.db') as db:
            confirmed = [(r,s) for r,s in db.execute("SELECT ref,status FROM receipts ORDER BY ref")]
            assert ('feishu:om_confirmed','confirmed') in confirmed
            assert ('feishu:om_wrong-id','failed') in confirmed
            assert db.execute("SELECT COUNT(*) FROM action_grants WHERE revoked_version>0 AND content_hash IS NOT NULL").fetchone()[0] >= 4
        assert sends == Counter({n: 1 for n in ('confirmed','wrong-id','lost-post','crash')}), sends
    finally:
        await app.kill()


def main():
    sends = Counter()
    crash_received = threading.Event()
    release = threading.Event()
    class Feishu(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def reply(self, payload):
            data = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_POST(self):
            packet = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if self.path.endswith('/internal'):
                self.reply(dict(code=0, tenant_access_token='fixture'))
                return
            name = packet['receive_id'].removeprefix('oc_')
            assert QUOTE in json.loads(packet['content'])['text']
            sends[name] += 1
            if name == 'crash':
                crash_received.set()
                release.wait(15)
                return
            if name != 'lost-post':
                self.reply(dict(code=0, data=dict(message_id='om_' + name)))

        def do_GET(self):
            message = self.path.rsplit('/',1)[-1]
            if message == 'om_wrong-id':
                message = 'om_other'
            self.reply(dict(code=0, data=dict(items=[dict(message_id=message)])))
    with tempfile.TemporaryDirectory() as tmp, ThreadingHTTPServer(('127.0.0.1',0), Feishu) as http:
        threading.Thread(target=http.serve_forever, daemon=True).start()
        try:
            asyncio.run(run(Path(tmp), f'http://127.0.0.1:{http.server_port}', sends, crash_received))
        finally:
            release.set()
            http.shutdown()
    print('PASS weekly report: scoped approval, source and budget checks, receipt read-back and crash recovery')

if __name__ == '__main__':
    main()
