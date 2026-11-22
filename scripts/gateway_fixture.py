"""Start the real Go gateway with a disposable loopback account."""
import contextlib
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
EMAIL = 'fixture@example.test'
PASSWORD = 'jeeves-test-account-password'
RUNTIME_TOKEN = 'web-fixture-token-private'


@contextlib.contextmanager
def gateway(runtime, home, token=RUNTIME_TOKEN):
    runtime_setting = 'GW_RUNTIME_HOME' if isinstance(runtime, Path) else 'GW_RUNTIME_URL'
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        listener_port = listener.getsockname()[1]
        address = f'127.0.0.1:{listener_port}'
    origin = 'http://' + address
    setup = secrets.token_hex(32)
    env = {k: v for k, v in os.environ.items() if not k.startswith('GW_')}
    env.update(
        GW_LOCAL='1', GW_ADDR=address, GW_PUBLIC_ORIGIN=origin,
        GW_SETUP_TOKEN=setup, GW_SESSION_KEY=secrets.token_hex(32),
        GW_STATE_KEY=secrets.token_hex(32),
        GW_RUNTIME_TOKEN=token,
        GW_DATABASE_URL='sqlite:' + str(Path(home) / f'fixture-accounts-{listener_port}.sqlite'),
        GW_DASHBOARD_DIR=str(ROOT / 'web/dist'),
    )
    env[runtime_setting] = str(runtime)
    log = Path(home) / 'gateway.log'
    with log.open('w') as stream:
        process = subprocess.Popen(
            [ROOT / 'target/jeeves-gateway'], env=env,
            stdout=stream, stderr=subprocess.STDOUT,
        )
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise RuntimeError(log.read_text())
                try:
                    with urllib.request.urlopen(origin + '/healthz', timeout=1):
                        break
                except OSError:
                    time.sleep(.1)
            else:
                raise RuntimeError('Gateway fixture did not become ready')
            request = urllib.request.Request(
                origin + '/api/setup',
                data=json.dumps(dict(email=EMAIL, password=PASSWORD, token=setup)).encode(),
                headers={'Content-Type': 'application/json'},
            )
            with urllib.request.urlopen(request, timeout=10) as response:
                cookie = response.headers['Set-Cookie'].split(';', 1)[0]
                workspace = '/app/' + json.load(response)['id'] + '/'
            yield origin, origin + workspace, cookie
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
