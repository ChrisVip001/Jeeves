"""Start the real Go gateway with a disposable loopback account."""
import contextlib
import json
import os
from pathlib import Path
import secrets
import socket
import ssl
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
EMAIL = 'fixture@example.test'
PASSWORD = 'jeeves-test-account-password'
RUNTIME_TOKEN = 'web-fixture-token-private'


def free_address():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return f'127.0.0.1:{listener.getsockname()[1]}'


@contextlib.contextmanager
def gateway(runtime, home, token=RUNTIME_TOKEN, mail_fixture=None):
    with contextlib.ExitStack() as stack:
        address = free_address()
        managed = mail_fixture is not None
        origin = ('https://' if managed else 'http://') + address
        tls = None
        setup = secrets.token_hex(32)
        env = {k: v for k, v in os.environ.items() if not k.startswith('GW_')}
        env.update(
            GW_LOCAL='1', GW_ADDR=address, GW_PUBLIC_ORIGIN=origin,
            GW_SETUP_TOKEN=setup, GW_SESSION_KEY=secrets.token_hex(32),
            GW_STATE_KEY=secrets.token_hex(32),
            GW_RUNTIME_TOKEN=token,
            GW_DATABASE_URL='sqlite:' + str(Path(home) / f"fixture-accounts-{address.split(':')[1]}.sqlite"),
            GW_DASHBOARD_DIR=str(ROOT / 'web/dist'),
        )
        if managed:
            control = free_address()
            control_key = secrets.token_hex(32)
            binary = home / 'control-plane'
            subprocess.run(['go', 'build', '-o', binary, './control-plane'], cwd=ROOT, check=True)
            control_env = {k:v for k,v in env.items() if not k.startswith(('CP_', 'GW_'))}
            control_env.update(CP_DEVELOPMENT='1', CP_ADDR=control,
                               CP_DATABASE_URL='sqlite:'+str(home / 'control.sqlite'),
                               CP_STATE_KEY=secrets.token_hex(32), CP_GATEWAY_TOKEN=control_key,
                               CP_TEST_RUNTIME_URL=runtime, CP_TEST_RUNTIME_TOKEN=token)
            service = subprocess.Popen([binary], env=control_env)
            stack.callback(service.wait, timeout=10)
            stack.callback(service.terminate)
            key, cert = home / 'tls.key', home / 'tls.crt'
            subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1',
                            '-subj','/CN=127.0.0.1','-addext','subjectAltName=IP:127.0.0.1',
                            '-keyout',key,'-out',cert], check=True, stderr=subprocess.DEVNULL)
            tls = ssl.create_default_context(cafile=str(cert))
            for field in ['GW_LOCAL','GW_SETUP_TOKEN','GW_RUNTIME_TOKEN']:
                del env[field]
            env.update(GW_DEVELOPMENT='1', GW_CONTROL_TOKEN=control_key, GW_CONTROL_URL='http://'+control,
                       GW_TLS_CERT=str(cert), GW_TLS_KEY=str(key), GW_MAIL_FROM=EMAIL,
                       GW_TEST_MAIL_URL=mail_fixture[0], GW_RESEND_API_KEY='fixture')
        else:
            runtime_setting = 'GW_RUNTIME_HOME' if isinstance(runtime, Path) else 'GW_RUNTIME_URL'
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
                        with urllib.request.urlopen(origin + '/healthz', timeout=1, context=tls):
                            break
                    except OSError:
                        time.sleep(.1)
                else:
                    raise RuntimeError('Gateway fixture did not become ready')
                def post(path, fields):
                    request = urllib.request.Request(origin + path, data=json.dumps(fields).encode(),
                                                     headers={'Content-Type': 'application/json'})
                    return urllib.request.urlopen(request, timeout=10, context=tls)
                if managed:
                    with post('/api/register', dict(email=EMAIL)) as response:
                        challenge = json.load(response)['challenge']
                    proof = challenge + '.' + mail_fixture[1].get(timeout=10)
                    path = '/api/verify'
                else:
                    path, proof = '/api/setup', setup
                with post(path, dict(email=EMAIL, password=PASSWORD, token=proof)) as response:
                    cookie = response.headers['Set-Cookie'].split(';', 1)[0]
                    workspace = '/app/' + json.load(response)['id'] + '/'
                yield origin, origin + workspace, cookie
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)
