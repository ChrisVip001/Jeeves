"""Run the shipped web workspace in an isolated iPhone Simulator WKWebView."""
import os
from pathlib import Path
import platform
import plistlib
import subprocess
import sys
import tempfile
import time
import uuid

device, workspace, cookie, task = sys.argv[1:]
shell = dict(os.environ)
if Path('/Applications/Xcode.app/Contents/Developer').is_dir():
    shell.setdefault('DEVELOPER_DIR', '/Applications/Xcode.app/Contents/Developer')
app_id = 'dev.jeeves.smoke.watch-' + uuid.uuid4().hex


def xcrun(*args, env=shell):
    return subprocess.check_output(['xcrun', *args], env=env, text=True).strip()


with tempfile.TemporaryDirectory() as folder:
    bundle = Path(folder) / 'WatchComposer.app'
    bundle.mkdir()
    sdk = xcrun('--sdk', 'iphonesimulator', '--show-sdk-path')
    xcrun('--sdk', 'iphonesimulator', 'swiftc', '-sdk', sdk, '-target', platform.machine()+'-apple-ios17.0-simulator',
          '-parse-as-library', str(Path(__file__).with_name('WatchComposer.swift')), '-o', str(bundle/'WatchComposer'))
    (bundle/'Info.plist').write_bytes(plistlib.dumps(dict(
        CFBundleIdentifier=app_id, CFBundleExecutable='WatchComposer', CFBundlePackageType='APPL',
        CFBundleName='Jeeves watch smoke', CFBundleVersion='1', CFBundleShortVersionString='1',
        MinimumOSVersion='17.0', UIDeviceFamily=[1], UILaunchScreen={},
        NSAppTransportSecurity={'NSAllowsLocalNetworking': True})))
    xcrun('simctl', 'install', device, str(bundle))
    try:
        data = Path(xcrun('simctl', 'get_app_container', device, app_id, 'data'))
        xcrun('simctl', 'launch', device, app_id, env=dict(shell, SIMCTL_CHILD_WATCH_WORKSPACE=workspace,
              SIMCTL_CHILD_WATCH_COOKIE=cookie, SIMCTL_CHILD_WATCH_TASK=task))
        result = data/'Documents/result'
        deadline = time.monotonic()+50
        while not result.exists() and time.monotonic()<deadline:
            time.sleep(.2)
        assert result.exists(), 'iOS composer timed out'
        assert result.read_text() == 'Submitted', result.read_text()
    finally:
        xcrun('simctl', 'uninstall', device, app_id)
