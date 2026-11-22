#!/usr/bin/env python3
"""Exercise the main composer, mobile amendment and a restart against the same quote source."""
import argparse
import importlib.util
import json
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import time

from gateway_fixture import gateway
import inline_delivery_fixture as shops

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('web', ROOT / 'scripts/smoke-web.py')
web = importlib.util.module_from_spec(spec)
spec.loader.exec_module(web)


def wait_for(read):
    deadline = time.monotonic() + 42
    while time.monotonic() < deadline:
        try:
            result = read()
            if result:
                return result
        except OSError:
            pass
        time.sleep(.15)
    raise AssertionError(f'Timed out waiting for {read}')


def run(home, channel, simulator):
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    base = f'http://127.0.0.1:{port}'
    provider, runtime = web.start(home, port)
    source = f'http://127.0.0.1:{provider.server_port}'
    shops.configure(home, source)
    try:
        wait_for(lambda: web.request(base, '/api/session/main'))
        with gateway(base, home) as (_, workspace, cookie), sqlite3.connect(home / 'state.db', isolation_level=None) as db:
            create = subprocess.run(['node', ROOT / 'scripts/smoke-inline-delivery.mjs', workspace, cookie,
                                     channel, source, home, 'watch'], stdout=subprocess.PIPE, text=True)
            assert create.returncode == 0, 'Desktop composer failed'
            assigned = json.loads(create.stdout)
            task = assigned['taskId']
            clock = db.execute('SELECT expires_ms,next_check_ms FROM price_watches WHERE task_id=?', (task,)).fetchone()
            assert clock
            created_ms = db.execute("SELECT unixepoch(created_at)*1000 FROM agent_tasks WHERE id=?", (task,)).fetchone()[0]
            assert abs(clock[0] - created_ms - 72*3600*1000) < 2000, clock
            if simulator:
                subprocess.run(['python3', ROOT / 'scripts/ios-watch-composer.py', simulator, workspace, cookie, task], check=True)
            else:
                subprocess.run(['node', ROOT / 'scripts/smoke-watch-amend.mjs', channel, workspace, cookie, task], check=True)
            row = db.execute('SELECT revision,threshold_minor,expires_ms,quote_json FROM price_watches WHERE task_id=?', (task,)).fetchone()
            assert row[:3] == (2, 170000, clock[0]), row
            assert json.loads(row[3]) == dict(url=source+'/shops/b', model='B', spec='256 GB', currency='CNY')
            assert db.execute('SELECT COUNT(*) FROM conversation_handoffs').fetchone() == (1,)
            assert db.execute('SELECT COUNT(*) FROM task_amendments').fetchone() == (1,)

            def restart(prepare=None):
                nonlocal runtime
                runtime.terminate()
                runtime.wait(timeout=15)
                if prepare:
                    prepare()
                runtime = web.start_runtime(home, port)
                wait_for(lambda: web.request(base, '/api/session/main'))

            def due_after_restart(price, shipping):
                def prepare():
                    shops.OFFERS['b'].update(price=price, shipping=shipping)
                    db.execute('UPDATE price_watches SET next_check_ms=0 WHERE task_id=?', (task,))
                restart(prepare)

            def observations():
                return db.execute('SELECT evidence FROM watch_observations WHERE task_id=? AND revision=2 ORDER BY checked_ms', (task,)).fetchall()

            due_after_restart(1660, 40)
            wait_for(lambda: any(json.loads(r[0]).get('totalMinor') == 170000 for r in observations()))
            assert db.execute("SELECT COUNT(*) FROM chat_log WHERE task_id=? AND source='task_completion'", (task,)).fetchone() == (0,)
            due_after_restart(1659, 40)
            def notices():
                history = web.request(base, '/api/session?sessionId=jeeves-main')['data']
                return [m for m in history if m.get('taskId') == task and m.get('source') == 'task_completion']
            receipts = wait_for(notices)
            assert len(receipts) == 1 and '1699.00 CNY' in str(receipts) and source+'/shops/b' in str(receipts), receipts
            restart()
            assert notices() == receipts
            assert db.execute("SELECT COUNT(*) FROM receipts WHERE kind='price_quote' AND status='confirmed'").fetchone() == (1,)
            tasks = web.request(base, '/api/tasks')['tasks']
            assert len(tasks)==1 and tasks[0]['id']==task and tasks[0]['checks'][0]['passed']
            assert not (home / 'receiver.jsonl').exists()
            print('PASS: one three-day watch, one amendment, same SKU and shipping basis, equality silent, one verified hit after restart, zero purchases.')
    finally:
        runtime.terminate()
        runtime.wait(timeout=15)
        provider.shutdown()


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--browser', default='chrome')
    parser.add_argument('--ios-simulator', help='Booted iPhone simulator UDID')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='jeeves-price-watch-') as home:
        run(Path(home), args.browser, args.ios_simulator)
