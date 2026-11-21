#!/usr/bin/env python3
"""Offline browser/task integration: real Runtime, owned worker and local fixtures only."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('desktop_smoke',ROOT/'scripts/smoke-desktop.py')
desktop=importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)

class Fixture(desktop.Provider):
    submitted=0
    uncertain=0
    def do_GET(self):
        if self.path=='/unknown':
            type(self).uncertain+=1
            time.sleep(18)
            try:self.send_response(200);self.end_headers();self.wfile.write(b'late response')
            except (BrokenPipeError,ConnectionResetError):pass
            return
        if self.path=='/saved':
            type(self).submitted+=1
            body=b'<body>Saved once successfully</body>'
        else:
            body=b"<body><h1>Browser fixture</h1><button onclick=\"location.href='/saved'\">Save</button><input type=password aria-label=Password></body>"
        self.send_response(200);self.send_header('Content-Type','text/html');self.end_headers();self.wfile.write(body)
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if not body.get('stream'):
            payload=json.dumps({'choices':[{'message':{'content':'{"retrieve":false,"query":"","reason":"fixture"}'}}]}).encode()
            self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(payload);return
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
        messages=body['messages']
        tool_calls=[call for message in messages for call in message.get('tool_calls',[])]
        if any('readonly-browser-fixture' in str(message.get('content','')) for message in messages if message['role']=='user'):
            if messages[-1]['role']=='tool':
                assert 'Read-only' in messages[-1]['content'],messages[-1]
                self.chunk({'content':'Read-only correctly denied.'},'stop')
            else:
                self.chunk({'tool_calls':[{'index':0,'id':'denied','type':'function','function':{'name':'browser_act','arguments':json.dumps({'kind':'scroll','observationId':'not-admitted','text':'down'})}}]})
                self.chunk({},'tool_calls')
            self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush();return

        unknown=any('unknown-browser-fixture' in str(message.get('content','')) for message in messages if message['role']=='user')
        if not tool_calls:
            name='browser_observe';args={}
        elif tool_calls[-1]['function']['name']=='browser_observe':
            output=messages[-1]['content']
            assert 'untrusted page data' in output,output
            state=json.loads(output.split(': ',1)[1])
            observation=state['observation']
            assert not any(item['name']=='Password' for item in observation['elements'])
            save=next(item for item in observation['elements'] if item['name']=='Save')
            name='browser_act'
            args=({'kind':'navigate','observationId':observation['id'],'url':observation['url'].rstrip('/')+'/unknown'} if unknown else {'kind':'click','observationId':observation['id'],'reference':save['reference'],'expectedName':'Save'})
        else:
            assert 'Saved once successfully' in messages[-1]['content'],messages[-1]
            self.chunk({'content':'Verified saved once.'},'stop');self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush();return
        self.chunk({'tool_calls':[{'index':0,'id':f'fixture-{len(tool_calls)}','type':'function','function':{'name':name,'arguments':json.dumps(args)}}]})
        self.chunk({},'tool_calls');self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush()


def run(binary=None,runtime=None):
    binary=Path(binary or ROOT/'jeeves-rs/target/debug/jeeves').resolve()
    runtime=Path(runtime or ROOT/'desktop/src-tauri/browser-runtime').resolve()
    assert (runtime/'runtime.json').is_file(),'Run just browser-runtime first'
    fixture=ThreadingHTTPServer(('127.0.0.1',0),Fixture)
    threading.Thread(target=fixture.serve_forever,daemon=True).start()
    with tempfile.TemporaryDirectory(prefix='jeeves-browser-smoke-') as home:
        rpc=desktop.Rpc(binary,home,{'JEEVES_BROWSER_RUNTIME':str(runtime),'JEEVES_BROWSER_HEADLESS':'1','PATH':'/usr/bin:/bin'})
        try:
            rpc.call('model/save',dict(id='browser-fixture',provider='ollama',model='fixture',smallModel='fixture',baseUrl=f'http://127.0.0.1:{fixture.server_port}/v1',makeActive=True))
            rpc.call('config/write',{'patch':{'consolidateEvery':1000,'graphWorkflows':False}})
            owner=rpc.call('session/main', {})['sessionId']
            assert rpc.call('session/permission/get', dict(sessionId=owner))['mode'] == 'standard'
            task=rpc.call('task/create',dict(sessionId=owner,title='Browser fixture'))['task']['id']
            row=rpc.call('browser/start',dict(taskId=task,url=f'http://127.0.0.1:{fixture.server_port}',origins=[],allowLocalhost=True))
            assert row['status']=='agent'
            approvals=[]
            def answer(event):
                if event['type']=='approvalRequested':
                    approvals.append(event)
                    assert event['tool']=='browser_act' and 'Save' in event['argsPreview'],event
                    rpc.call('approval/respond',dict(turnId=event['turnId'],requestId=event['requestId'],approved=True,remember='once'))
            rpc.until(rpc.send('task/start',dict(taskId=task,message='Save through the task browser and verify')),answer)
            # Dispatch returns before the task completes; keep following the Runtime's live stream.
            deadline=time.monotonic()+30
            while time.monotonic()<deadline:
                current=rpc.until(rpc.send('task/get',dict(taskId=task)),answer)[0]['task']
                if current['status'] in ('done','failed','needsAttention','needs_attention') and current['dispatchState']=='finished':break
                time.sleep(.05)
            assert current['status']=='done',current
            assert Fixture.submitted==1,Fixture.submitted
            assert len(approvals)==1,approvals
            frame=rpc.call('browser/frame',dict(taskId=task));assert frame['dataUrl'].startswith('data:image/jpeg;base64,')
            row=rpc.call('browser/list',dict(sessionId=owner))['sessions'][0]
            assert row['url'].endswith('/saved')
            stale=row['generation']
            paused=rpc.call('browser/control',dict(taskId=task,generation=stale,action='pause'))
            try:rpc.call('browser/control',dict(taskId=task,generation=stale,action='return'))
            except AssertionError as error:assert 'refresh' in str(error)
            else:raise AssertionError('Stale controller accepted')
            assert paused['status']=='paused'
            rpc.call('session/permission/set',dict(sessionId=owner,mode='readOnly',expectedRevision=1,confirmFullAccess=False))
            row=rpc.call('browser/list',dict(sessionId=owner))['sessions'][0];assert row['status']=='paused'
            with sqlite3.connect(Path(home)/'state.db') as db:
                source=db.execute("SELECT source FROM chat_log WHERE task_id=?",(task,)).fetchall()
                assert source and all(item[0]=='browser-task' for item in source),source
                operations=db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '%operation%'").fetchall()
                assert operations
            rpc.call('browser/control',dict(taskId=task,generation=row['generation'],action='close'))
            denied=rpc.call('task/create',dict(sessionId=owner,title='Read-only browser fixture'))['task']['id']
            rpc.call('browser/start',dict(taskId=denied,url=f'http://127.0.0.1:{fixture.server_port}',allowLocalhost=True))
            rpc.call('task/start',dict(taskId=denied,message='readonly-browser-fixture'))
            for _ in range(100):
                denied_row=rpc.call('task/get',dict(taskId=denied))['task']
                if denied_row['dispatchState']=='finished':break
                time.sleep(.05)
            assert denied_row['status']=='done',denied_row
            assert Fixture.submitted==1
            assert not any(event.get('tool')=='browser_act' and event not in approvals for event in rpc.notifications if event['type']=='approvalRequested')
            unknown_task=rpc.call('task/create',dict(sessionId=owner,title='Unknown effect fixture'))['task']['id']
            rpc.call('session/permission/set',dict(sessionId=owner,mode='fullAccess',expectedRevision=2,confirmFullAccess=True))
            rpc.call('browser/start',dict(taskId=unknown_task,url=f'http://127.0.0.1:{fixture.server_port}',allowLocalhost=True))
            rpc.call('task/start',dict(taskId=unknown_task,message='unknown-browser-fixture'))
            deadline=time.monotonic()+25
            while time.monotonic()<deadline:
                unknown_row=rpc.call('task/get',dict(taskId=unknown_task))['task']
                if unknown_row['dispatchState']=='finished':break
                time.sleep(.1)
            assert unknown_row['status'] in ('needsAttention','needs_attention','suspended'),unknown_row
            assert Fixture.uncertain==1
            operation=rpc.call('task/operations',dict(taskId=unknown_task))
            assert operation['operations'],operation
            try:rpc.call('task/resume',dict(taskId=unknown_task))
            except AssertionError as error:assert 'unknown' in str(error) or 'reconcil' in str(error)
            else:raise AssertionError('Unknown browser effect resumed without human evidence')
            assert Fixture.uncertain==1
            # Leave this context open. Restart must mark it stopped rather than resurrect it.

        finally:rpc.close()
        # A Runtime restart never recreates browser contexts, grants or inputs.
        rpc=desktop.Rpc(binary,home,{'JEEVES_BROWSER_RUNTIME':str(runtime),'JEEVES_BROWSER_HEADLESS':'1','PATH':'/usr/bin:/bin'})
        try:
            rows=rpc.call('browser/list',dict(sessionId=owner))['sessions'];assert next(row for row in rows if row['taskId']==task)['status']=='closed'
            assert next(row for row in rows if row['taskId']==unknown_task)['status']=='needsAttention'
            assert Fixture.uncertain==1
            assert next(row for row in rows if row['taskId']==denied)['status']=='needsAttention'
            assert Fixture.submitted==1
        finally:rpc.close()
        emergency_stop(binary, home, runtime, fixture)
    fixture.shutdown();fixture.server_close()
    print('Browser smoke passed: owned worker → fresh refs → once approval → one submission → verification → pause/close/restart; read-only and unknown-effect replay refused')

def emergency_stop(binary, home, runtime, fixture):
    env = {'JEEVES_BROWSER_RUNTIME':str(runtime),'JEEVES_BROWSER_HEADLESS':'1','PATH':'/usr/bin:/bin'}
    rpc = desktop.Rpc(binary, home, env)
    url = f'http://127.0.0.1:{fixture.server_port}'
    before = Fixture.uncertain
    try:
        owner = rpc.call('session/main', {})['sessionId']
        policy = rpc.call('session/permission/get', dict(sessionId=owner))
        if policy['mode'] != 'fullAccess':
            rpc.call('session/permission/set',dict(sessionId=owner,mode='fullAccess',expectedRevision=policy['revision'],confirmFullAccess=True))
        def open_task(title):
            task = rpc.call('task/create',dict(sessionId=owner,title=title))['task']['id']
            return rpc.call('browser/start',dict(taskId=task,url=url,allowLocalhost=True))
        first = open_task('Paused locally')
        second = open_task('Emergency stop during navigation')
        rpc.call('browser/control',dict(taskId=first['taskId'],generation=first['generation'],action='pause'))
        assert rpc.call('browser/frame',dict(taskId=second['taskId']))['dataUrl']
        rpc.call('task/start',dict(taskId=second['taskId'],message='unknown-browser-fixture'))
        deadline = time.monotonic()+15
        while Fixture.uncertain == before and time.monotonic() < deadline:
            time.sleep(.05)
        assert Fixture.uncertain == before+1, 'navigation did not reach the local fixture'
        start = time.monotonic()
        stopped = rpc.call('executor/stop',{})
        assert time.monotonic()-start < 5, 'stop waited for the worker’s 12-second navigation timeout'
        assert stopped['stopped'] and rpc.call('executor/stop',{}) == stopped
        rows = rpc.call('browser/list',dict(sessionId=owner))['sessions']
        active_ids = {first['taskId'], second['taskId']}
        assert active_ids == {r['taskId'] for r in rows if r['taskId'] in active_ids and r['status']=='needsAttention' and not r['observedAt']}, rows
        deadline = time.monotonic()+10
        while time.monotonic() < deadline:
            task = rpc.call('task/get',dict(taskId=second['taskId']))['task']
            if task['dispatchState']=='finished':
                break
            time.sleep(.05)
        assert task['status'] in ('needsAttention','needs_attention','suspended'),task
        ops = rpc.call('task/operations',dict(taskId=second['taskId']))['operations']
        assert any(op['tool']=='browser_act' and op['status']=='intent' and op['replayPolicy']=='never' for op in ops),ops
    finally:
        rpc.close()
    rpc = desktop.Rpc(binary,home,env)
    try:
        assert rpc.call('executor/control',{}) == stopped
        params = dict(taskId=first['taskId'],url=url,allowLocalhost=True)
        try:
            rpc.call('browser/start',params)
            raise AssertionError('fresh launch accepted while stopped')
        except AssertionError as error:
            assert 'Executors are stopped' in str(error),error
        assert not rpc.call('executor/release',dict(expectedGeneration=stopped['generation']))['stopped']
        fresh = rpc.call('browser/start',params)
        assert fresh['id'] != first['id'] and fresh['observedAt'] and fresh['status']=='agent'
        assert Fixture.uncertain == before+1, 'old navigation was replayed'
        print('PASS global stop: two live browsers, interrupted input, duplicate stop, restart and fresh observation')
    finally:
        rpc.close()


if __name__=='__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('--binary',type=Path)
    parser.add_argument('--runtime',type=Path)
    args=parser.parse_args()
    run(args.binary,args.runtime)
