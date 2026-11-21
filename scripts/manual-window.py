#!/usr/bin/env python3
"""Run a disposable Mac window target, a local screen-model stub and a separate cockpit."""
import json
import os
from pathlib import Path
from gateway_fixture import EMAIL, PASSWORD, RUNTIME_TOKEN, gateway
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / 'jeeves-rs/target/debug'


class ScreenModel(BaseHTTPRequestHandler):
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        has_image = any(
            block.get('type') == 'image_url'
            for message in request['messages'] if isinstance(message['content'], list)
            for block in message['content']
        )
        text = '测试端点已收到本次确认的图片。' if has_image else '本机模拟模型已完成本轮响应。'
        if request.get('stream'):
            chunks = [
                {'index': 0, 'delta': {'content': text}, 'finish_reason': None},
                {'index': 0, 'delta': {}, 'finish_reason': 'stop'},
            ]
            events = ['data: ' + json.dumps({'choices': [chunk]}) for chunk in chunks]
            body = ('\n\n'.join([*events, 'data: [DONE]']) + '\n\n').encode()
            content_type = 'text/event-stream'
        else:
            body = json.dumps({'choices': [{'message': {'role': 'assistant', 'content': text},
                                            'finish_reason': 'stop'}]}).encode()
            content_type = 'application/json'
        self.send_response(200)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def prepare(home, port, binary=BIN / 'jeeves'):
    env = {k: os.environ[k] for k in ('HOME', 'PATH', 'LANG', 'TMPDIR') if k in os.environ}
    env.update(JEEVES_HOME=str(home), JEEVES_PORT='17884', JEEVES_OPS_TOKEN=RUNTIME_TOKEN,
               JEEVES_DESKTOP_HELPER=str(BIN / 'jeeves-window'))
    profile = 'window-fixture'
    (home / 'models.json').write_text(json.dumps({'active': profile, 'profiles': [
        {'id': profile, 'provider': 'ollama', 'model': 'fixture', 'vision': True,
         'base_url': f'http://127.0.0.1:{port}/v1'}]}))
    with subprocess.Popen([binary, 'app-server'], cwd=home, env=env, text=True,
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE) as engine:
        try:
            def call(seq, method, params):
                engine.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': seq, 'method': method, 'params': params}) + '\n')
                engine.stdin.flush()
                for line in engine.stdout:
                    message = json.loads(line)
                    if message.get('id') == seq:
                        if 'error' in message:
                            raise RuntimeError(message['error']['message'])
                        return message['result']
                raise RuntimeError('Fixture setup lost its engine')
            owner = call(1, 'session/main', {})['sessionId']
            task = call(2, 'task/create', {
                'sessionId': owner, 'title': 'Mac 单窗口验收 / Window acceptance',
            })['task']['id']
            call(3, 'desktop/model', {'taskId': task, 'profileId': profile})
        finally:
            engine.stdin.close()
    return env


def main():
    with tempfile.TemporaryDirectory(prefix='jeeves-window-') as tmp, ThreadingHTTPServer(('127.0.0.1', 0), ScreenModel) as model:
        threading.Thread(target=model.serve_forever, daemon=True).start()
        home = Path(tmp)
        env = prepare(home, model.server_port)
        with subprocess.Popen([BIN / 'JeevesWindowFixture'], cwd=home) as windows:
            try:
                with gateway('http://127.0.0.1:17884', home) as (origin, _, _), \
                        subprocess.Popen([BIN / 'jeeves', 'runtime'], cwd=home, env=env) as cockpit:
                    print(f'打开 {origin}，用 {EMAIL} / {PASSWORD} 登录。', flush=True)
                    print('两个 Mac 窗口供测试操作。点击「主会话 → 任务进展」，保留任务的待开始状态，在「Mac 单窗口验收 / Window acceptance」任务卡展开「任务窗口」，点击「在 Mac 上选择窗口」。按 Ctrl-C 结束测试。', flush=True)
                    try:
                        if cockpit.wait() != 0:
                            raise RuntimeError('The test cockpit exited before acceptance finished')
                    finally:
                        if cockpit.poll() is None:
                            cockpit.terminate()
            finally:
                windows.terminate()


if __name__ == '__main__':
    try:
        main()
    except KeyboardInterrupt:
        pass
