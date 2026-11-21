#!/usr/bin/env python3
"""Offline main conversation → independent tasks → human approval → continuation.

Uses the shipped HTTP adapters and real runtime, with an isolated home and a local
scripted provider. No account or paid model is involved.
"""
import importlib.util
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
from gateway_fixture import RUNTIME_TOKEN, gateway
import queue
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request
import urllib.error

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('web_smoke', ROOT / 'scripts/smoke-web.py')
web = importlib.util.module_from_spec(spec)
spec.loader.exec_module(web)


def interact(base, message, message_id, task=None):
    params = dict(sessionId='jeeves-main', message=message, messageId=message_id,
                  taskId=task, cwd=None, attachments=None)
    request = urllib.request.Request(base + '/api/interaction', data=json.dumps(params).encode(),
                                     headers={'Authorization': 'Bearer ' + RUNTIME_TOKEN, 'Content-Type': 'application/json'})
    events = []
    with urllib.request.urlopen(request, timeout=30) as response:
        for line in response:
            if line.startswith(b'data: '):
                events.append(json.loads(line[6:]))
    assert not any(e['type'] == 'error' for e in events), events
    assert any(e['type'] == 'turnCompleted' for e in events), events
    return events


def tasks(base):
    return web.request(base, '/api/tasks')['tasks']


def settled(base, task, minimum_runs=0):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        row = next(t for t in tasks(base) if t['id'] == task)
        if row['runCount'] >= minimum_runs and row['status'] in ('done', 'failed', 'cancelled') and row['dispatchState'] == 'finished':
            return row
        time.sleep(.05)
    raise AssertionError('Task did not settle')


def run(browser_channel=None):
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix='jeeves-interaction-smoke-') as home:
        provider, process = web.start(home, port)
        base = f'http://127.0.0.1:{port}'
        events = queue.Queue()
        response = None
        reader = None
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise AssertionError(Path(home, "server.log").read_text())
                try:
                    web.request(base, '/api/model/catalog')
                    break
                except OSError:
                    time.sleep(.1)
            else:
                raise AssertionError('Runtime did not start')
            assert web.request(base, '/api/session/main')['sessionId'] == 'jeeves-main'
            interact(base, 'hello fixture', 'hello')
            with ThreadPoolExecutor(max_workers=2) as clients:
                messages = ['simultaneous-a', 'simultaneous-b']
                responses = [clients.submit(interact, base, 'same message text', key) for key in messages]
                for key, future in zip(messages, responses):
                    completion = next(event for event in future.result() if event['type'] == 'turnCompleted')
                    assert completion['turnId'] == 'interaction-' + key, completion
            assert not Path(home, 'workspaces').exists(), 'chat should allocate no task folder'
            web.request(base, '/api/session/permission', dict(sessionId='jeeves-main', mode='fullAccess', expectedRevision=1, confirmFullAccess=True))
            interact(base, 'task-alpha-fixture;task-beta-fixture', 'two-tasks')
            rows = tasks(base)
            assert len(rows) == 2
            assert rows[0]['cwd'] != rows[1]['cwd']
            for row in rows:
                row = settled(base, row['id'])
                assert row['status'] == 'done', row
                path = Path(row['cwd'])
                assert path.is_relative_to(Path(home).resolve() / 'workspaces')
                assert (path / 'report.txt').read_text() == row['title'], ((path / 'report.txt').read_text(), row['title'])
            interact(base, 'task-alpha-fixture;task-beta-fixture', 'two-tasks')
            assert len(tasks(base)) == 2, 'retry dispatched duplicate work'
            sessions = web.request(base, '/api/data')['sessions']
            assert all(not s['id'].startswith('task-work:') for s in sessions)
            alpha = next(row for row in rows if row['title'] == 'task-alpha-fixture')
            if browser_channel:
                with gateway(base, home) as (_, workspace, cookie):
                    subprocess.run(['node', ROOT / 'scripts/smoke-task-receipt.mjs', workspace, cookie, alpha['id'], browser_channel], check=True)
            else:
                interact(base, 'task-follow-fixture', 'follow', alpha['id'])
            fresh = settled(base, alpha['id'], minimum_runs=2)
            assert fresh['cwd'] == alpha['cwd'] and fresh['executionSessionId'] == alpha['executionSessionId']
            assert fresh['runCount'] == 2
            assert Path(alpha['cwd'], 'report.txt').read_text() == 'task-follow-fixture'
            history = web.request(base, '/api/session?sessionId=jeeves-main')['data']
            receipts = [m for m in history if m.get('source') == 'task_completion']
            assert sorted(m['taskId'] for m in receipts) == sorted([alpha['id'], *(t['id'] for t in rows)])
            assert all(m['receiptStatus'] == 'produced_unverified' for m in receipts)
            artifacts = web.request(base, '/api/task', {'action': 'artifacts', 'taskId': alpha['id']})
            assert artifacts['latestDelivery']['status'] == 'produced_unverified' and artifacts['latestDelivery']['runId'], artifacts
            assert artifacts['latestDelivery']['summary'], artifacts
            assert artifacts['artifacts'][0]['writtenText'] == 'task-follow-fixture', artifacts
            process.terminate()
            process.wait(timeout=12)
            provider.shutdown()
            provider.server_close()
            provider, process = web.start(home, port)
            for _ in range(100):
                try:
                    restored = web.request(base, '/api/task', {'action': 'artifacts', 'taskId': alpha['id']})
                    break
                except OSError:
                    time.sleep(.1)
            else:
                raise AssertionError('Runtime did not recover')
            assert restored == artifacts, (restored, artifacts)

            # Independent GET subscription must carry worker approvals after the POST closes.
            request = urllib.request.Request(base + '/api/notifications', headers={'Authorization': 'Bearer ' + RUNTIME_TOKEN})
            response = urllib.request.urlopen(request, timeout=15)
            def receive():
                try:
                    for line in response:
                        if line.startswith(b'data: '):
                            events.put(json.loads(line[6:]))
                except (OSError, ValueError):
                    pass
            reader = threading.Thread(target=receive, daemon=True)
            reader.start()
            web.request(base, '/api/session/permission', dict(sessionId='jeeves-main', mode='standard', expectedRevision=2, confirmFullAccess=False))
            interact(base, 'task-follow-fixture approved', 'approved-follow', alpha['id'])
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                event = events.get(timeout=10)
                if event['type'] == 'approvalRequested':
                    break
            else:
                raise AssertionError('Task approval did not reach the subscriber')
            # A newly opened client recovers the same live question without its SSE history.
            pending_path = '/api/approval/pending?sessionId=jeeves-main'
            try:
                web.request(base, pending_path, auth=False)
                raise AssertionError('pending approvals must require authentication')
            except urllib.error.HTTPError as error:
                assert error.code == 401
            restored = web.request(base, pending_path)['approvals']
            assert len(restored) == 1 and restored[0]['request']['requestId'] == event['requestId']
            assert restored[0]['task']['id'] == alpha['id']
            assert 0 < restored[0]['request']['expiresInMs'] <= event['expiresInMs']
            other = 'outside-main'
            try:
                web.request(base, '/api/session/new', {})
                raise AssertionError('public session creation must be unavailable')
            except urllib.error.HTTPError as error:
                assert error.code in (404, 405)
            try:
                pending_outside = web.request(base, '/api/approval/pending?sessionId=' + other)
                assert not pending_outside['approvals']
            except urllib.error.HTTPError as error:
                assert error.code == 400
            params = dict(turnId=event['turnId'], requestId=event['requestId'], approved=True, remember='once')
            if browser_channel:
                with gateway(base, home) as (_, workspace, cookie):
                    subprocess.run(['node', ROOT / 'scripts/smoke-task-receipt.mjs', workspace, cookie, alpha['id'], browser_channel, event['requestId']], check=True)
            else:
                assert web.request(base, '/api/approval', params)['accepted']
            assert not web.request(base, '/api/approval', params)['accepted']
            assert settled(base, alpha['id'])['runCount'] == 3
            assert not web.request(base, pending_path)['approvals']
            assert Path(alpha['cwd'], 'report.txt').read_text() == 'task-follow-fixture approved'
            # Ordinary turns and goal verification both produce durable task timeline events.
            verified_ids = []
            for index in range(3):
                verified = web.request(base, '/api/task', {'action': 'create', 'sessionId': 'jeeves-main', 'title': f'Timeline verification fixture {index}', 'cwd': alpha['cwd'], 'acceptance': [{'kind': 'fileExists', 'path': 'report.txt'}], 'maxRounds': 1})['task']
                web.request(base, '/api/task', {'action': 'start', 'taskId': verified['id'], 'message': 'Verify the existing local report'})
                assert settled(base, verified['id'])['status'] == 'done'
                verified_ids.append(verified['id'])
            timeline_path = '/api/task/events?'
            complete = web.request(base, timeline_path + 'cursor=0&limit=500')['events']
            replay, cursor = [], 0
            alpha_kinds = {event['kind'] for event in complete if event['taskId'] == alpha['id']}
            assert {'run_started', 'run_finished', 'approval_requested', 'approval_decided'} <= alpha_kinds, alpha_kinds
            while True:
                page = web.request(base, timeline_path + f'cursor={cursor}&limit=2')
                replay.extend(page['events'])
                assert page['nextCursor'] > cursor or not page['hasMore']
                cursor = page['nextCursor']
                if not page['hasMore']:
                    break
            assert len(replay) > 2 and replay == complete, (len(replay), len(complete), [event['kind'] for event in replay], [event['kind'] for event in complete])
            assert len({event['cursor'] for event in replay}) == len(replay)
            assert not web.request(base, timeline_path + f'cursor={cursor}')['events']
            for task_id in verified_ids:
                assert web.request(base, timeline_path + f'taskId={task_id}')['events'] == [event for event in replay if event['taskId'] == task_id]
            print('Main interaction smoke passed: chat, split, isolated files, retry, history, continuation, artifacts, task approval, reconnect snapshot, timeline replay.', flush=True)
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            if reader:
                reader.join(timeout=5)
            if response:
                response.close()
            provider.shutdown()
            provider.server_close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--browser', choices=['chrome', 'chromium'], help='submit the follow-up from the phone layout using this installed Playwright channel')
    run(parser.parse_args().browser)
