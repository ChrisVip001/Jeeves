#!/usr/bin/env python3
"""Offline desktop RPC smoke: real app-server, local scripted provider, isolated home."""
import argparse
import json
import os
from pathlib import Path
import queue
import runpy
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


PROMOTION_RECEIPT = "本地模拟渠道已收到最终文案，回执 om_synthetic。"


class Provider(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if not body.get('stream'):
            content = '{"retrieve":false,"query":"","reason":"fixture"}'
            if any('JEEVES_INTERACTION_ROUTER_V1' in str(m.get('content', '')) for m in body['messages'] if m['role'] == 'system'):
                value = json.loads(body['messages'][-1]['content'])
                message = value['message']
                if 'task-alpha-fixture' in message:
                    items = [part.strip() for part in message.split(';') if part.strip()]
                    content = json.dumps({'kind': 'tasks', 'items': [{'title': part, 'instruction': part} for part in items]})
                else:
                    content = '{"kind":"conversation"}'
            payload = json.dumps({'choices': [{'message': {'content': content}, 'finish_reason': 'stop'}], 'usage': {'prompt_tokens': 5, 'completion_tokens': 5}}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(payload)
            return
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        messages = body['messages']
        user = next((m.get('content', '') for m in reversed(messages) if m['role'] == 'user'), '')
        if isinstance(user, list):
            user = ' '.join(b.get('text', '') for b in user)
        tool_result = messages[-1]['role'] == 'tool'
        try:
            instruction = user.rsplit('Current human task:', 1)[-1].strip()
            task_tool = any(t.get('function', {}).get('name') == 'task' for t in body.get('tools', []))
            if 'open-source-promotion-fixture' in user and not tool_result:
                final = 'final edition' in user
                revised = 'second edition' in user
                if task_tool and 'final edition #' in user:
                    task_id = user.rsplit('#',1)[1].strip()
                    name, args = 'task', {'action':'continue','task_id':task_id,'instruction':user}
                elif task_tool:
                    name, args = 'task', {'action':'create','items':[{'title':'Open source promotion','instruction':user}]}
                else:
                    copy = ('Final edition: ' if final else 'Second edition: ' if revised else 'First edition: ') + 'Synthetic open source announcement. '*10 + ('End of revised copy.' if final else 'End of first copy.')
                    name, args = 'feishu_send', {'to':'oc_final' if final else 'oc_first','text':copy}
                self.chunk({'tool_calls':[{'index':0,'id':'promotion-final' if final else 'promotion-second' if revised else 'promotion-first','type':'function','function':{'name':name,'arguments':json.dumps(args)}}]})
                self.chunk({},'tool_calls')
            elif tool_result and 'Sent to oc_final (Feishu message_id om_synthetic)' in str(messages[-1].get('content', '')):
                self.chunk({'content': PROMOTION_RECEIPT}, 'stop')
            elif task_tool and 'task-alpha-fixture' in user and not tool_result:
                items = [{'title': part.strip(), 'instruction': part.strip()} for part in user.split(';') if part.strip()]
                self.chunk({'tool_calls': [{'index': 0, 'id': 'fixture-dispatch', 'type': 'function', 'function': {'name': 'task', 'arguments': json.dumps({'action': 'create', 'items': items})}}]})
                self.chunk({}, 'tool_calls')
            elif ('task-alpha-fixture' in user or 'task-beta-fixture' in user or 'task-follow-fixture' in user) and not tool_result:
                self.chunk({'tool_calls': [{'index': 0, 'id': 'fixture-file', 'type': 'function', 'function': {'name': 'write_file', 'arguments': json.dumps({'path': 'report.txt', 'content': instruction})}}]})
                self.chunk({}, 'tool_calls')
            elif 'custom-connector-fixture' in user and not tool_result:
                if any(t['function']['name'] == 'custom_commit' for t in body.get('tools', [])):
                    self.chunk({'tool_calls': [{'index': 0, 'id': 'fixture-custom', 'type': 'function', 'function': {'name': 'custom_commit', 'arguments': '{"value":"synthetic"}'}}]})
                    self.chunk({}, 'tool_calls')
                else:
                    self.chunk({'content': 'Custom tool unavailable.'}, 'stop')
            elif 'approval-fixture' in user and not tool_result:
                self.chunk({'tool_calls': [{'index': 0, 'id': 'fixture-call', 'type': 'function', 'function': {'name': 'memory', 'arguments': '{"action":"save","subject":"desktop-fixture","content":"approved once"}'}}]})
                self.chunk({}, 'tool_calls')
            else:
                for text in ['桌面', '连接', '测试', '完成。']:
                    self.chunk({'content': text})
                    time.sleep(.35 if 'slow-fixture' in user else .01)
                self.chunk({}, 'stop')
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass

    def chunk(self, delta, finish=None):
        self.wfile.write(('data: ' + json.dumps({'choices': [{'index': 0, 'delta': delta, 'finish_reason': finish}]}) + '\n\n').encode())
        self.wfile.flush()


class Rpc:
    def __init__(self, binary, home, extra_env=None, *, base_env=None, cwd=None):
        inherited = os.environ if base_env is None else base_env
        env = {k: v for k, v in inherited.items() if not k.startswith('JEEVES_')}
        env.update(JEEVES_HOME=str(home), JEEVES_PROVIDER='ollama', JEEVES_SEARCH_DDG='0')
        env.update(extra_env or {})
        self.proc = subprocess.Popen([str(binary), 'app-server'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env, cwd=cwd)
        self.frames = queue.Queue()
        self.next_id = 0
        self.notifications = []
        self.responses = {}
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        for line in self.proc.stdout:
            try:
                self.frames.put(json.loads(line))
            except json.JSONDecodeError:
                self.frames.put({'error': {'message': 'Malformed app-server frame'}})
        self.frames.put({'error': {'message': 'app-server disconnected'}})

    def send(self, method, params):
        self.next_id += 1
        self.proc.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': self.next_id, 'method': method, 'params': params}) + '\n')
        self.proc.stdin.flush()
        return self.next_id

    def until(self, request_id, callback=None):
        start = len(self.notifications)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if request_id in self.responses:
                return self.responses.pop(request_id), self.notifications[start:]
            frame = self.frames.get(timeout=max(.1, deadline - time.monotonic()))
            if 'error' in frame:
                raise AssertionError(frame['error'])
            if frame.get('method') == 'turn/notification':
                event = frame['params']
                self.notifications.append(event)
                if callback:
                    callback(event)
            elif 'id' in frame:
                self.responses[frame['id']] = frame['result']
            else:
                raise AssertionError('Unexpected frame')
        raise AssertionError('RPC deadline exceeded')

    def call(self, method, params):
        return self.until(self.send(method, params))[0]

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=5)
        self.proc.stdout.close()


def check_window_fixture(binary):
    fixture = runpy.run_path(str(Path(__file__).with_name('manual-window.py')))
    with ThreadingHTTPServer(('127.0.0.1', 0), fixture['ScreenModel']) as model:
        threading.Thread(target=model.serve_forever, daemon=True).start()
        try:
            with tempfile.TemporaryDirectory(prefix='jeeves-window-smoke-') as home:
                fixture['prepare'](Path(home), model.server_port, binary)
                rpc = Rpc(binary, home)
                try:
                    owner = rpc.call('session/main', {})['sessionId']
                    tasks = [t for t in rpc.call('task/list', {})['tasks'] if t['sessionId'] == owner]
                    assert [t['title'] for t in tasks] == ['Mac 单窗口验收 / Window acceptance']
                    task_id = tasks[0]['id']
                    rpc.call('task/start', {'taskId': task_id, 'message': 'Test the local model'})
                    deadline = time.monotonic() + 15
                    while True:
                        task = rpc.call('task/get', {'taskId': task_id})['task']
                        if task['status'] not in ('pending', 'running') or time.monotonic() >= deadline:
                            break
                        time.sleep(.1)
                    assert task['status'] == 'done', (task['status'], task.get('lastStopReason'))
                finally:
                    rpc.close()
        finally:
            model.shutdown()


def run(binary):
    check_window_fixture(binary)
    provider = ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix='jeeves-desktop-smoke-') as home:
            rpc = Rpc(binary, home)
            try:
                rpc.call('model/save', {'id': 'desktop-fixture', 'provider': 'ollama', 'model': 'fixture', 'smallModel': 'fixture', 'baseUrl': f'http://127.0.0.1:{provider.server_port}/v1', 'makeActive': True})
                rpc.call('config/write', {'patch': {'consolidateEvery': 1000, 'graphWorkflows': False}})
                session = rpc.call('session/main', {})['sessionId']
                policy = rpc.call('session/permission/get', {'sessionId': session})
                assert policy['mode'] == 'standard' and policy['revision'] == 1
                _, events = rpc.until(rpc.send('turn/start', {'sessionId': session, 'message': 'hello fixture', 'stream': True}))
                assert any(e['type'] == 'textDelta' for e in events)
                assert any(e['type'] == 'turnCompleted' and e['reply'] == '桌面连接测试完成。' for e in events)
                def run_task(message, on_event=None):
                    task = rpc.call('task/create', {'title': message})['task']['id']
                    start = len(rpc.notifications)
                    rpc.until(rpc.send('task/start', {'taskId': task, 'message': message}), on_event)
                    deadline = time.monotonic() + 12
                    while time.monotonic() < deadline:
                        result, _ = rpc.until(rpc.send('task/get', {'taskId': task}), on_event)
                        if result['task']['dispatchState'] == 'finished':
                            return rpc.notifications[start:]
                        time.sleep(.05)
                    raise AssertionError(f'Worker did not finish {message}')

                asked = []
                def answer(event):
                    if event['type'] == 'approvalRequested':
                        asked.append(event)
                        rpc.call('approval/respond', {'turnId': event['turnId'], 'requestId': event['requestId'], 'approved': True, 'remember': 'once'})
                events = run_task('approval-fixture', answer)
                assert len(asked) == 1, asked
                assert any(e['type'] == 'toolCompleted' and not e['output'].startswith('Error:') for e in events)
                rpc.call('session/permission/set', {'sessionId': session, 'mode': 'readOnly', 'expectedRevision': 1, 'confirmFullAccess': False})
                events = run_task('approval-fixture')
                assert not any(e['type'] == 'approvalRequested' for e in events)
                assert any(e['type'] == 'toolCompleted' and 'Read-only' in e['output'] for e in events)
                interrupted = []
                def interrupt(event):
                    if event['type'] == 'textDelta' and not interrupted:
                        turn = next(e['turnId'] for e in captured if e['type'] == 'turnStarted')
                        interrupted.append(rpc.call('turn/interrupt', {'turnId': turn}))
                captured = []
                def observe(event):
                    captured.append(event)
                    interrupt(event)
                _, events = rpc.until(rpc.send('turn/start', {'sessionId': session, 'message': 'slow-fixture', 'stream': True}), observe)
                assert interrupted and interrupted[0]['interrupted']
                assert any(e['type'] == 'turnCompleted' and e['meta']['interrupted'] for e in events)
            finally:
                rpc.close()
            rpc = Rpc(binary, home)
            try:
                assert rpc.call('session/permission/get', {'sessionId': session})['mode'] == 'readOnly'
                assert len(rpc.call('session/messages', {'sessionId': session, 'limit': 100})['data']) >= 4
            finally:
                rpc.close()
    finally:
        provider.shutdown()
        provider.server_close()
    print('Desktop smoke passed: streaming, approval, read-only, interruption, history and policy restart.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parents[1] / 'jeeves-rs/target/debug/jeeves')
    run(parser.parse_args().binary.resolve())
