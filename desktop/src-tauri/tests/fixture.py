#!/usr/bin/env python3
import json
import os
import sys
import time

for line in sys.stdin:
    req = json.loads(line)
    if req['method'] == 'session/list':
        sys.stdout.write('{"method":"turn/notification","params":{"type":"test"}}\n')
        response = json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': {'data': []}}) + '\n'
        sys.stdout.write(response[:12])
        sys.stdout.flush()
        sys.stdout.write(response[12:])
        sys.stdout.flush()
    elif req['method'] == 'browser/list':
        print(json.dumps({'jsonrpc':'2.0','id':req['id'],'result':{'browserRuntime':os.environ.get('JEEVES_BROWSER_RUNTIME')}}),flush=True)
    elif req['method'] == 'session/new':
        print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'error': {'message': 'fixture error'}}), flush=True)
    elif req['method'] == 'turn/list':
        print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': {'turns': []}}), flush=True)
    elif req['method'] == 'session/update':
        print('malformed frame', flush=True)
    elif req['method'] == 'model/list':
        sys.stderr.write('x' * 1024 * 1024)
        sys.stderr.flush()
        print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': {'data': []}}), flush=True)
    elif req['method'] == 'model/profiles':
        print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': {}}), flush=True)
        time.sleep(30)
    elif req['method'] == 'session/messages':
        break
