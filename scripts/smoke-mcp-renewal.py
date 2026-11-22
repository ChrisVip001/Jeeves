#!/usr/bin/env python3
"""Resident OAuth renewal against a loopback authorization server, across app-server restarts."""
import json
import os
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs

ROOT = Path(__file__).resolve().parents[1]
class Provider(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers['Content-Length']))
        if self.path == '/token':
            fields = parse_qs(body.decode())
            assert fields['grant_type'] == ['refresh_token']
            self.server.refreshes.append(fields['refresh_token'][0])
            mode = self.server.mode
            status, reply = {
                'ok': (200, {'access_token':'fixture-access-new', 'refresh_token':'fixture-rotated', 'expires_in':3600}),
                'offline': (503, {'error':'fixture-sensitive-error'}),
                'revoked': (400, {'error':'invalid_grant', 'error_description':'fixture-sensitive-error'}),
            }[mode]
        else:
            data = json.loads(body)
            if 'id' not in data:
                self.send_response(202); self.end_headers(); return
            method = data['method']
            if method == 'tools/call':
                self.server.calls.append(method)
            status = 200
            reply = {'jsonrpc':'2.0', 'id':data['id'], 'result':{'tools':[]} if method=='tools/list' else {'protocolVersion':'2025-06-18','capabilities':{},'serverInfo':{'name':'local','version':'1'}}}
        raw = json.dumps(reply).encode()
        self.send_response(status); self.send_header('Content-Type','application/json')
        self.send_header('Content-Length', str(len(raw))); self.end_headers(); self.wfile.write(raw)

def until(predicate):
    deadline = time.monotonic() + 35
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.1)
    raise AssertionError('resident OAuth state did not converge')

def run():
    http = ThreadingHTTPServer(('127.0.0.1',0), Provider)
    http.mode, http.refreshes, http.calls = 'ok', [], []
    threading.Thread(target=http.serve_forever,daemon=True).start()
    base = 'http://127.0.0.1:' + str(http.server_port)
    binary = ROOT / 'target/debug/jeeves'
    try:
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            (home/'mcp.json').write_text(json.dumps({'servers':[{'name':'fixture','url':base+'/mcp','oauth':True}]}))
            auth = home/'mcp-auth/fixture.json'
            auth.parent.mkdir()
            def save(access, refresh, expiry):
                data = {'resource_url':base+'/mcp','token_endpoint':base+'/token','client_info':{'client_id':'fixture'},
                        'tokens':{'access_token':access,'refresh_token':refresh,'expires_at':int(time.time())+expiry}}
                tmp = auth.with_suffix('.fixture')
                tmp.write_text(json.dumps(data)); tmp.chmod(0o600); tmp.replace(auth)
            def tokens():
                return json.loads(auth.read_text()).get('tokens')
            save('fixture-access-old','fixture-original',3600)
            def start():
                return subprocess.Popen([str(binary),'app-server'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=log,
                    env={'PATH':os.environ['PATH'],'HOME':directory,'JEEVES_HOME':directory},text=True)
            def call(proc, method, params):
                proc.stdin.write(json.dumps(dict(jsonrpc='2.0',id=1,method=method,params=params))+'\n'); proc.stdin.flush()
                for line in proc.stdout:
                    frame=json.loads(line)
                    if frame.get('id')==1:
                        return frame['result']
                raise AssertionError('fixture runtime exited')
            with (home/'log').open('w') as log:
                proc=start()
                try:
                    call(proc,'config/read',{})
                    save('fixture-access-old','fixture-original',30)
                    until(lambda: tokens()['access_token']=='fixture-access-new')
                    assert auth.stat().st_mode & 0o777 == 0o600
                    proc.kill(); proc.wait(); proc=start()
                    call(proc,'config/read',{})
                    assert http.refreshes == ['fixture-original']
                    http.mode='offline'; save('fixture-access-new','fixture-rotated',20)
                    until(lambda: len(http.refreshes)>=2)
                    assert tokens()['refresh_token']=='fixture-rotated'
                    http.mode='revoked'
                    until(lambda: tokens() is None)
                    card=call(proc,'capability/probe',{'id':'mcp:fixture'})['card']
                    assert card['state']=='unauthorized' and http.calls==[],card
                    assert http.refreshes[-1]=='fixture-rotated'
                finally:
                    proc.kill(); proc.wait()
            output=(home/'log').read_text()
            assert all(s not in output for s in ['fixture-access-old','fixture-access-new','fixture-original','fixture-rotated','fixture-sensitive-error'])
            print('PASS idle refresh, 0600 rotation, restart, transient failure, invalid_grant and no business call')
    finally:
        http.shutdown(); http.server_close()
if __name__=='__main__':
    run()
