#!/usr/bin/env python3
"""Offline Web smoke through authenticated HTTP and the real Rust runtime."""
import importlib.util
import json
import os
import re
import socket
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import urllib.parse

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('desktop_smoke', ROOT / 'scripts/smoke-desktop.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


def start(home, port, extra_env=None):
    provider = fixture.ThreadingHTTPServer(('127.0.0.1', 0), fixture.Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    binary = Path(os.environ['CARGO_TARGET_DIR']) / 'debug/jeeves' if os.environ.get('CARGO_TARGET_DIR') else ROOT / 'jeeves-rs/target/debug/jeeves'
    rpc = fixture.Rpc(binary, home)
    try:
        rpc.call('model/save', {'id': 'local-fixture', 'provider': 'ollama', 'model': 'fixture', 'smallModel': 'fixture', 'baseUrl': f'http://127.0.0.1:{provider.server_port}/v1', 'makeActive': True})
        rpc.call('config/write', {'patch': {'consolidateEvery': 1000, 'graphWorkflows': False}})
    finally:
        rpc.close()
    env = {k: v for k, v in os.environ.items() if not k.startswith('JEEVES_')}
    env.update(JEEVES_HOME=str(home), JEEVES_PORT=str(port), JEEVES_PROVIDER='ollama', JEEVES_OPS_TOKEN='web-fixture-token', JEEVES_DASHBOARD_DIR=str(ROOT / 'jeeves-ts/packages/dashboard/dist'), JEEVES_WRITE='1', JEEVES_SEARCH_DDG='0')
    env.update(extra_env or {})
    with (Path(home) / 'server.log').open('w') as log:
        process = subprocess.Popen([str(binary), 'dashboard'], env=env, stdout=log, stderr=subprocess.STDOUT)
    return provider, process


def request(base, path, body=None, auth=True):
    headers = {'Authorization': 'Bearer web-fixture-token'} if auth else {}
    if body is not None:
        headers['Content-Type'] = 'application/json'
    req = urllib.request.Request(base + path, data=json.dumps(body).encode() if body is not None else None, headers=headers)
    with urllib.request.urlopen(req, timeout=20) as response:
        return json.load(response)


def stream(base, session, message, approve=False):
    req = urllib.request.Request(base + '/api/turn', data=json.dumps({'sessionId': session, 'message': message, 'stream': True}).encode(), headers={'Authorization': 'Bearer web-fixture-token', 'Content-Type': 'application/json'})
    events = []
    with urllib.request.urlopen(req, timeout=20) as response:
        for line in response:
            if not line.startswith(b'data: '):
                continue
            event = json.loads(line[6:])
            events.append(event)
            if approve and event['type'] == 'approvalRequested':
                params = {'turnId': event['turnId'], 'requestId': event['requestId'], 'approved': True, 'remember': 'once'}
                assert request(base, '/api/approval', params)['accepted']
                assert not request(base, '/api/approval', params)['accepted']
    return events


def task_stream(base, session, message, approve=False):
    req = urllib.request.Request(base + '/api/notifications',
                                 headers={'Authorization': 'Bearer web-fixture-token'})
    events = []
    with urllib.request.urlopen(req, timeout=20) as response:
        task = request(base, '/api/task', {'action': 'create', 'sessionId': session, 'title': message})['task']
        request(base, '/api/task', {'action': 'start', 'taskId': task['id'], 'message': message})
        for line in response:
            if not line.startswith(b'data: '):
                continue
            event = json.loads(line[6:])
            events.append(event)
            if approve and event['type'] == 'approvalRequested':
                params = {'turnId': event['turnId'], 'requestId': event['requestId'], 'approved': True, 'remember': 'once'}
                assert request(base, '/api/approval', params)['accepted']
                assert not request(base, '/api/approval', params)['accepted']
            if event['type'] == 'turnCompleted':
                return events
    raise AssertionError('Task completed without its receipt')


def custom_connector(base, session, home):
    recovery = importlib.util.spec_from_file_location('recovery', ROOT / 'scripts/smoke-recovery.py')
    module = importlib.util.module_from_spec(recovery)
    recovery.loader.exec_module(module)
    server = Path(home) / 'fixture.py'
    server.write_text(module.MCP_PROGRAM)
    spec = dict(name='custom', command=sys.executable, args=[str(server), str(home), 'normal'], createOnly=True)
    def visible():
        return any(c['id'] == 'mcp:custom' for c in request(base, '/api/capability')['cards'])
    assert not visible()
    assert request(base, '/api/mcp/save', spec)['ok']
    assert visible(), 'saved connection missing from a previously cached list'
    row = request(base, '/api/connector/tools?id=mcp:custom')
    assert row == {'tools':[{'name':'commit','effect':'write','disabled':False}], 'editable':True}, row
    switch = dict(id='mcp:custom', tool='commit')
    for disabled in [True, False]:
        current = request(base, '/api/connector/tools/set', dict(switch, disabled=disabled))
        assert current['tools'][0]['disabled'] == disabled
        events = task_stream(base, session, 'custom-connector-fixture', approve=True)
        asks = [e for e in events if e['type'] == 'approvalRequested']
        if disabled:
            assert not asks and not (Path(home) / 'commits').exists()
            assert any('Custom tool unavailable.' in str(e) for e in events)
        else:
            assert len(asks) == 1 and 'synthetic' in asks[0]['argsPreview']
            assert any(e['type'] == 'toolCompleted' and 'receipt local-1' in e['output'] for e in events)
            assert module.count(Path(home) / 'commits') == 1
    assert request(base, '/api/mcp/remove', {'name':'custom'})['ok']
    assert not visible()


def device_pairing(base):
    # The fixture signs with Node's Ed25519 implementation, independently of the Rust verifier.
    node = """
      const {generateKeyPairSync,sign} = require('node:crypto');
      const {publicKey,privateKey} = generateKeyPairSync('ed25519');
      const deviceId = Buffer.from(publicKey.export({format:'jwk'}).x,'base64url').toString('base64');
      console.log(deviceId);
      require('node:readline').createInterface({input:process.stdin}).on('line', line => {
        console.log(sign(null,Buffer.from(line),privateKey).toString('base64'));
      });
    """
    with subprocess.Popen(['node','-e',node], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True) as signer:
        try:
            device_id = signer.stdout.readline().strip()
            params = dict(deviceId=device_id, capabilities=['browserControl','approvalReview'], expectedGeneration=0)
            for path, body in [('pair', params), ('revoke', dict(deviceId=device_id, expectedGeneration=1))]:
                try:
                    request(base, '/api/device/' + path, body, auth=False)
                    raise AssertionError('device administration needs the cockpit credential')
                except urllib.error.HTTPError as error:
                    assert error.code == 401
            grant = request(base, '/api/device/pair', params)
            report = dict(deviceId=device_id, grantId=grant['grantId'], generation=grant['generation'],
                          capability='browserControl', sequence=1)
            payload = ['jeeves.device.report.v1', device_id, grant['grantId'], grant['generation'], 'browserControl', 1]
            signer.stdin.write(json.dumps(payload, separators=(',', ':')) + '\n')
            signer.stdin.flush()
            report['signature'] = signer.stdout.readline().strip()
            assert request(base, '/api/device/report', report, auth=False) == {'sequence':1}
            assert request(base, '/api/device')['devices'][0]['lastSequence'] == 1
            locator = dict(deviceId=device_id, grantId=grant['grantId'], generation=1, taskId='fixture', turnId='turn', approvalRequestId='ask', approvalVersion=1)
            read = ['jeeves.approval.notice.v1', device_id, grant['grantId'], 1, 'fixture', 'turn', 'ask', 1, None]
            signer.stdin.write(json.dumps(read, separators=(',', ':')) + '\n')
            signer.stdin.flush()
            locator['signature'] = signer.stdout.readline().strip()
            path = '/api/approval/notice?' + urllib.parse.urlencode(locator)
            for endpoint, body in [(path, None), ('/api/approval/notice', dict(locator, approved=True)), ('/api/approval/push-key', {})]:
                try:
                    request(base, endpoint, body, auth=False)
                    raise AssertionError('approval notice route requires cockpit authentication')
                except urllib.error.HTTPError as error:
                    assert error.code == 401
            assert request(base, '/api/approval/push-key', {}) == {'publicKey': None}
            assert request(base, path) == {'status': 'expired', 'approval': None}
            assert request(base, '/api/device/revoke', dict(deviceId=device_id, expectedGeneration=1))['revoked']
            assert request(base, path) == {'status': 'revoked', 'approval': None}
            try:
                request(base, '/api/device/report', report, auth=False)
                raise AssertionError('revoked report accepted')
            except urllib.error.HTTPError as error:
                assert error.code == 400
        finally:
            signer.stdin.close()


def seed_mail_followups(base, home):
    import sqlite3
    session = request(base, '/api/session/main')['sessionId']
    tasks = []
    for label in ('travel', 'review'):
        task = request(base, '/api/task', {'action':'create', 'sessionId':session,
                       'title':f'Mail fixture {label}', 'maxRounds':1})['task']
        tasks.append(task['id'])
        with sqlite3.connect(Path(home) / 'state.db') as db:
            db.execute("INSERT INTO task_dispatch(task_id,session_id,turn_id,message,state) VALUES(?,?,?,?, 'finished')",
                       (task['id'], f'task-work:{label}', f'fixture:{label}', task['title']))
            db.execute("INSERT INTO mail_actions(id,task_id,account,envelope,state) VALUES(?,?,?,?,'queued')",
                       (f'mail-{label}', task['id'], 'fixture', json.dumps({'subject': label, 'body': f'{label} fixture body'})))
    return tasks


def run(port=0, hold=False, mail_followup=False):
    if not port:
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix='jeeves-web-smoke-') as home:
        provider, process = start(home, port)
        base = f'http://127.0.0.1:{port}'
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise AssertionError('dashboard exited before becoming ready: ' + (Path(home) / 'server.log').read_text()[-2000:])
                startup = (Path(home) / 'server.log').read_text(encoding='utf-8')
                ready = re.search(r'Jeeves cockpit → (http://[^\s]+)', startup)
                if ready:
                    base = ready[1]
                try:
                    if not ready:
                        time.sleep(.1)
                        continue
                    request(base, '/api/model/catalog')
                    break
                except (OSError, urllib.error.URLError):
                    time.sleep(.1)
            else:
                raise AssertionError('dashboard startup timed out')
            try:
                request(base, '/api/session/permission?sessionId=missing', auth=False)
                raise AssertionError('permission API must require authentication')
            except urllib.error.HTTPError as error:
                assert error.code == 401
            try:
                request(base, '/api/connector/catalog', auth=False)
                raise AssertionError('catalog requires cockpit authentication')
            except urllib.error.HTTPError as error:
                assert error.code == 401
            for route, body in [('input', {}), ('download', {}), ('downloads?taskId=test', None)]:
                try:
                    request(base, '/api/browser/' + route, body, auth=False)
                    raise AssertionError('remote browser operations require cockpit authentication')
                except urllib.error.HTTPError as error:
                    assert error.code == 401
            for route, body in [('control', None), ('stop', {}), ('release', {'expectedGeneration': 1})]:
                try:
                    request(base, '/api/executor/' + route, body, auth=False)
                    raise AssertionError('executor controls require cockpit authentication')
                except urllib.error.HTTPError as error:
                    assert error.code == 401
            assert not request(base, '/api/executor/control')['stopped']
            stop = request(base, '/api/executor/stop', {})
            assert stop['stopped'] and stop == request(base, '/api/executor/stop', {})
            assert not request(base, '/api/executor/release', {'expectedGeneration':stop['generation']})['stopped']
            device_pairing(base)
            catalog = request(base, '/api/connector/catalog')['entries']
            assert len(catalog) == 20
            mail = next(row for row in catalog if row['id'] == 'email')
            assert mail['manifest']['verbs']['mail.send'] == 'email_send'
            manifest = {'category': 'tasks', 'tools': {'list': {'effect': 'read', 'resource': 'fixture'}},
                        'verbs': {'tasks.list': 'list'}}
            server = dict(name='fixture-tasks', url='http://127.0.0.1:1', manifest=manifest)
            assert request(base, '/api/mcp/save', server)['ok']
            saved = (Path(home) / 'mcp.json').read_bytes()
            duplicate = dict(server, createOnly=True, command='unused')
            duplicate.pop('url')
            assert 'already exists' in str(request(base, '/api/mcp/save', duplicate))
            assert (Path(home) / 'mcp.json').read_bytes() == saved
            for bad_server in [
                dict(name='bad', url='https://fixture.invalid/path?access_token=fixture'),
                dict(name='bad', url='http://fixture.invalid', auth_env='FIXTURE_KEY'),
                dict(name='bad', url='https://fixture.invalid', auth_env='FIXTURE_KEY', oauth=True),
                dict(name='bad', command='unused', oauth=True),
            ]:
                result = request(base, '/api/mcp/save', bad_server)
                assert not result.get('ok', False) and 'access_token=fixture' not in str(result)
                assert (Path(home) / 'mcp.json').read_bytes() == saved
            invalid = dict(manifest, tools={'list': {}, 'unknown': {'effect': 'read'}})
            assert not request(base, '/api/mcp/save', dict(server, manifest=invalid)).get('ok', False)
            assert (Path(home) / 'mcp.json').read_bytes() == saved
            card = request(base, '/api/capability/probe', {'id': 'mcp:fixture-tasks'})['card']
            assert card['manifest']['category'] == 'tasks' and card['manifest']['verbs'] == manifest['verbs']
            assert card['manifest']['tools']['list']['resource'] == 'fixture'
            tools_url = '/api/connector/tools?id=mcp:fixture-tasks'
            before = request(base, tools_url)
            assert before['editable'] and before['tools'] == [{'name':'list', 'effect':'read', 'disabled':False}], before
            switch = {'id':'mcp:fixture-tasks', 'tool':'list', 'disabled':True}
            assert request(base, '/api/connector/tools/set', switch)['tools'][0]['disabled']
            assert request(base, tools_url)['tools'][0]['disabled']
            assert not request(base, '/api/connector/tools/set', dict(switch, disabled=False))['tools'][0]['disabled']
            assert request(base, '/api/mcp/remove', {'name': 'fixture-tasks'})['ok']
            duties = '/api/responsibilities'
            contract = {'id': 'fixture-duty', 'purpose': 'Summarize fixture notes', 'sources': ['fixture:notes'],
                        'cron': '0 9 * * 1', 'expiresAt': '2099-01-01T00:00:00Z', 'budget': {'tools': 2}}
            assert request(base, duties)['responsibilities'] == []
            for bad_contract, authenticated, status in [
                (contract, False, 401),
                (dict(contract, budget={'tokens': 12, 'tools': 0}), True, 400),
                (dict(contract, budget={'tokens': 12, 'toolz': 1}), True, 400),
            ]:
                try:
                    request(base, duties, bad_contract, auth=authenticated)
                    raise AssertionError('invalid or unauthorized responsibility accepted')
                except urllib.error.HTTPError as error:
                    assert error.code == status
            assert request(base, duties)['responsibilities'] == []
            assert request(base, duties, contract)['status'] == 'active'
            for paused, expected in [(True, 'paused'), (False, 'active')]:
                assert request(base, duties + '/pause', {'id': contract['id'], 'paused': paused})['status'] == expected
            assert request(base, duties)['responsibilities'][0]['contract']['budget'] == {'tools': 2}
            session = request(base, '/api/session/main')['sessionId']
            policy = request(base, '/api/session/permission?sessionId=' + session)
            assert policy['mode'] == 'standard'
            custom_connector(base, session, home)
            params = {'sessionId': session, 'mode': 'fullAccess', 'expectedRevision': 1, 'confirmFullAccess': False}
            try:
                request(base, '/api/session/permission', params)
                raise AssertionError('full access must require confirmation')
            except urllib.error.HTTPError as error:
                assert error.code == 400
            config = request(base, '/api/config')['config']
            assert config['provider'] == 'ollama'
            request(base, '/api/config', {'patch': {'maxTokens': 4096}})
            try:
                request(base, '/api/config', {'patch': {'maxTokens': 0, 'maxIterations': 40}})
                raise AssertionError('invalid config must reject the whole patch')
            except urllib.error.HTTPError as error:
                assert error.code == 400
            assert request(base, '/api/config')['config']['maxIterations'] == config['maxIterations']
            events = stream(base, session, 'hello fixture')
            assert any(e['type'] == 'textDelta' for e in events)
            assert any(e['type'] == 'turnCompleted' for e in events)
            events = task_stream(base, session, 'approval-fixture', approve=True)
            assert sum(e['type'] == 'approvalRequested' for e in events) == 1
            assert any(e['type'] == 'toolCompleted' and e['status'] == 'ok' for e in events)
            request(base, '/api/session/permission', {'sessionId': session, 'mode': 'readOnly', 'expectedRevision': 1, 'confirmFullAccess': False})
            events = task_stream(base, session, 'approval-fixture')
            assert not any(e['type'] == 'approvalRequested' for e in events)
            assert any(e['type'] == 'toolCompleted' and 'Read-only' in e['output'] for e in events)
            assert len(request(base, '/api/session?sessionId=' + session)['data']) >= 6
            with urllib.request.urlopen(base + '/', timeout=10) as response:
                html = response.read().decode()
                assert 'composer-context' in html and 'main.js?v=' in html and './style.css' in html
            with urllib.request.urlopen(base + '/style.css', timeout=10) as response:
                assert b'--jeeves-background' in response.read()
            if mail_followup:
                mail_tasks = seed_mail_followups(base, home)
                print('Mail fixture tasks ' + json.dumps(mail_tasks), flush=True)
            print(f'Web smoke passed; {base} (token web-fixture-token)', flush=True)
            if hold:
                input()
                if mail_followup:
                    import sqlite3
                    with sqlite3.connect(Path(home) / 'state.db') as db:
                        assert db.execute("SELECT id,state FROM mail_actions WHERE account='fixture' ORDER BY id").fetchall() == [
                            ('mail-review', 'queued'), ('mail-travel', 'draft')]

        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            provider.shutdown()
            provider.server_close()


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('--port', type=int, default=0)
    parser.add_argument('--hold', action='store_true')
    parser.add_argument('--mail-followup', action='store_true')
    args = parser.parse_args()
    run(args.port, args.hold, args.mail_followup)
