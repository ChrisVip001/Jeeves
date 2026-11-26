#!/usr/bin/env python3
"""Operator-only remote input and real Chromium transfers against a loopback fixture."""
import base64
import importlib.util
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('desktop_smoke', ROOT / 'scripts/smoke-desktop.py')
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class Page(BaseHTTPRequestHandler):
    count = 0
    finish_transfer = threading.Event()
    uploaded = None
    upload_received = threading.Event()

    def do_POST(self):
        if self.path != '/upload':
            self.send_error(404)
            return
        Page.uploaded = self.rfile.read(int(self.headers['Content-Length']))
        Page.upload_received.set()
        self.send_response(204)
        self.end_headers()

    def do_GET(self):
        if self.path == '/blocked-redirect':
            self.send_response(302)
            self.send_header('Location', 'https://example.com/')
            self.end_headers()
            return
        if self.path.startswith('/file'):
            Page.count += 1
            body = f'Fixture report {Page.count}'.encode()
            self.send_response(200)
            self.send_header('Content-Disposition', 'attachment; filename="report.txt"')
            self.send_header('Content-Type', 'application/octet-stream')
        elif self.path == '/slow':
            self.send_response(200)
            self.send_header('Content-Disposition','attachment; filename="receiving.txt"')
            self.send_header('Content-Length','4096')
            self.end_headers()
            self.wfile.write(bytes(2048))
            self.wfile.flush()
            self.finish_transfer.wait(8)
            try:
                self.wfile.write(bytes(2048))
            except (BrokenPipeError, ConnectionResetError):
                pass
            return
        elif self.path == '/large':
            body = bytes(900 * 1024)
            self.send_response(200)
            self.send_header('Content-Disposition', 'attachment; filename="large.bin"')
        else:
            self.send_response(200)
            self.send_header('Content-Type', 'text/html')
            body = b"""<title>Remote fixture</title><style>*{margin:0}a,input,button{display:block;height:48px;width:250px}</style>
            <a href="/file">Download report</a><a href="/large">Oversize</a><input aria-label="Name">
            <button onclick="document.title=document.querySelector('input').value">Use name</button><a href="/slow">Slow download</a>
            <input type="file" onchange="fetch('/upload',{method:'POST',body:this.files[0]})">"""
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def require_denied(rpc, method, params):
    try:
        rpc.call(method, params)
    except AssertionError:
        return
    raise AssertionError(f'{method} unexpectedly succeeded')


def run():
    server = ThreadingHTTPServer(('127.0.0.1', 0), Page)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    origin = f'http://127.0.0.1:{server.server_port}'
    installed = ROOT/'desktop/src-tauri/browser-runtime'
    binary = ROOT/'target/debug/jeeves'
    try:
        with tempfile.TemporaryDirectory(prefix='jeeves-remote-browser-') as temp:
            runtime = Path(temp)/'runtime'
            runtime.mkdir()
            for name in ['node','runtime.json','browsers']:
                (runtime/name).symlink_to(installed/name)
            # Test the checkout’s worker using the already-installed matching Node and Chromium.
            (runtime/'worker').symlink_to(ROOT/'executors/browser')
            home = Path(temp)/'home'
            rpc = desktop.Rpc(binary,home,{'JEEVES_BROWSER_RUNTIME':str(runtime),'JEEVES_BROWSER_HEADLESS':'1'})
            try:
                owner = rpc.call('session/main', {})['sessionId']
                rejected = rpc.call('task/create', dict(sessionId=owner, title='Blocked first navigation'))['task']['id']
                require_denied(rpc, 'browser/start', dict(taskId=rejected, url=origin+'/blocked-redirect', allowLocalhost=True))
                failed = next(row for row in rpc.call('browser/list', dict(sessionId=owner))['sessions'] if row['taskId'] == rejected)
                assert failed['status'] == 'needsAttention' and 'ERR_' in failed['error'], failed
                def open_task(title):
                    task = rpc.call('task/create',dict(sessionId=owner,title=title))['task']['id']
                    row = rpc.call('browser/start',dict(taskId=task,url=origin,allowLocalhost=True))
                    assert not rpc.call('browser/frame',dict(taskId=task)).get('frameId')
                    active = rpc.call('browser/control',dict(taskId=task,generation=row['generation'],action='takeover'))
                    return dict(taskId=task,generation=active['generation'])
                task = open_task('Collect report')
                other = open_task('Separate browser')
                def frame():
                    return rpc.call('browser/frame',dict(taskId=task['taskId']))['frameId']
                def act(snapshot,**input_):
                    return rpc.call('browser/input',dict(**task,frameId=snapshot,input=input_))['frameId']
                old = frame()
                next_frame = act(old,kind='click',x=30,y=122)
                require_denied(rpc,'browser/input',dict(**task,frameId=old,input={'kind':'text','text':'replay'}))
                next_frame = act(next_frame,kind='text',text='Fixture Operator')
                next_frame = act(next_frame,kind='key',key='tab')
                act(next_frame,kind='key',key='enter')
                selected = rpc.call('browser/input', dict(**task, frameId=frame(), input=dict(kind='click', x=30, y=253)))
                assert selected['upload']['origin'] == origin
                png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/e1EAAAAASUVORK5CYII='
                upload = dict(kind='upload', chooserId=selected['upload']['chooserId'], filename='test.png', data=png)
                require_denied(rpc, 'browser/input', dict(**other, frameId=selected['frameId'], input=upload))
                act(selected['frameId'], **upload)
                assert Page.upload_received.wait(5) and Page.uploaded == base64.b64decode(png)
                require_denied(rpc, 'browser/input', dict(**task, frameId=selected['frameId'], input=upload))
                def item(y,status):
                    seen = {r['id'] for r in rpc.call('browser/downloads',dict(taskId=task['taskId']))['downloads']}
                    act(frame(),kind='click',x=30,y=y)
                    deadline=time.monotonic()+5
                    while time.monotonic()<deadline:
                        rows=rpc.call('browser/downloads',dict(taskId=task['taskId']))['downloads']
                        for row in rows:
                            if row['id'] not in seen and row['status']==status: return row
                        time.sleep(.04)
                    raise AssertionError((status,rows))
                for expected in [1,2]:
                    report=item(20,'pending')
                    require_denied(rpc,'browser/download',dict(**other,downloadId=report['id'],action='save'))
                    save=dict(**task,downloadId=report['id'],action='save')
                    for _ in range(2):
                        saved=rpc.call('browser/download',save)['downloads']
                        assert any(r['id']==report['id'] and r['status']=='saved' for r in saved)
                    artifacts=rpc.call('task/artifacts',dict(taskId=task['taskId']))['artifacts']
                    assert len(artifacts)==expected
                    path=Path(artifacts[0]['path'])
                    assert path.parent==home/'artifacts'/task['taskId'] and path.read_text()==f'Fixture report {expected}'
                cancel=item(20,'pending')
                rows=rpc.call('browser/download',dict(**task,downloadId=cancel['id'],action='cancel'))['downloads']
                assert any(r['id']==cancel['id'] and r['status']=='cancelled' for r in rows)
                require_denied(rpc,'browser/download',dict(**task,downloadId=cancel['id'],action='save'))
                receiving=item(220,'receiving')
                rows=rpc.call('browser/download',dict(**task,downloadId=receiving['id'],action='cancel'))['downloads']
                assert any(r['id']==receiving['id'] and r['status']=='cancelled' for r in rows),rows
                Page.finish_transfer.set()
                assert item(67,'failed')['detail']
                assert not rpc.call('task/artifacts',dict(taskId=other['taskId']))['artifacts']
                assert len(rpc.call('task/artifacts',dict(taskId=task['taskId']))['artifacts'])==2
                old=frame()
                returned=rpc.call('browser/control',dict(**task,action='return'))
                assert returned['observedAt'] and returned['title']=='Fixture Operator'
                require_denied(rpc,'browser/input',dict(**task,frameId=old,input={'kind':'key','key':'enter'}))
                assert Page.count==3
            finally:
                rpc.close()
    finally:
        Page.finish_transfer.set()
        server.shutdown()
        server.server_close()
    print('PASS remote browser: fresh frame, human input, one scoped image upload, return, two task scopes, duplicate filenames, repeat save, cancellation and oversize failure')


if __name__ == '__main__':
    run()
