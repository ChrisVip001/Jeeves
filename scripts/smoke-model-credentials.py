"""Crash the disposable stdio runtime at a credential write boundary and check its next boot."""
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import time


def fixture_helper():
    """Only installed in the private PATH passed to this test's subprocess."""
    root = Path(os.environ["FIXTURE_DIR"])
    op, *args = sys.argv[1:]
    mac = Path(sys.argv[0]).name == "security"
    account = args[args.index("-a" if mac else "account") + 1]
    store = root / "items"
    items = json.loads(store.read_text()) if store.exists() else {}
    if op in ("find-generic-password", "lookup"):
        if account not in items:
            return 44 if mac else 1
        (root / "resolved").write_text(items[account])
        print(items[account])
        return 0

    armed = root / "armed"

    def pause(point):
        if armed.exists() and armed.read_text() == point:
            (root / "paused").touch()
            deadline = time.monotonic() + 4
            while armed.exists() and time.monotonic() < deadline:
                time.sleep(0.01)

    if op in ("add-generic-password", "store"):
        pause("before-store")
        items[account] = args[args.index("-w") + 1] if mac else sys.stdin.read().rstrip("\n")
        store.write_text(json.dumps(items))
        pause("stored")
        (root / "helper-stored").touch()
    elif op in ("delete-generic-password", "clear"):
        pause("before-delete")
        if account not in items:
            return 44 if mac else 1
        del items[account]
        store.write_text(json.dumps(items))
        pause("deleted")
    else:
        return 2
    return 0


def exercise(binary, helper_dir, root, scenario):
    home = root / "home"
    home.mkdir(parents=True)
    journal = home / ".jeeves-model-change.json"
    armed = root / "armed"
    found = root / "resolved"
    keys = ("fixture-value-before", "fixture-value-after")
    output = root / "stderr"
    env = dict(JEEVES_HOME=str(home), PATH=str(helper_dir), FIXTURE_DIR=str(root), TMPDIR=str(root))

    def start():
        with output.open("a") as log:
            return subprocess.Popen([binary, "app-server"], cwd=home, env=env, stdin=subprocess.PIPE,
                                    stdout=subprocess.PIPE, stderr=log, text=True)

    def stop(proc):
        proc.stdin.close()
        try:
            assert proc.wait(timeout=8) == 0
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
            proc.stdout.close()

    def read_reply(proc):
        def timed_out(*_):
            raise TimeoutError(f"{scenario}: runtime did not reply")
        previous = signal.signal(signal.SIGALRM, timed_out)
        signal.alarm(12)
        try:
            for line in proc.stdout:
                assert all(key not in line for key in keys), "credential in RPC output"
                response = json.loads(line)
                if response.get("id") == 1:
                    assert "result" in response, response
                    return response["result"]
            raise AssertionError("runtime closed before replying")
        finally:
            signal.alarm(0)
            signal.signal(signal.SIGALRM, previous)

    def request(proc, method, params, wait=True):
        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=1, method=method, params=params)) + "\n")
        proc.stdin.flush()
        if wait:
            return read_reply(proc)

    def save(proc, index, wait=True):
        return request(proc, "model/save", dict(id="fixture", provider="openai",
                       model=("first", "second")[index], apiKey=keys[index]), wait)

    setup = scenario not in ("create", "late-store")
    point = {"create": "stored", "rotate": "stored", "published": "before-delete", "remove": "deleted", "late-store": "before-store"}[scenario]
    proc = start()
    try:
        request(proc, "model/profiles", {})
        if setup:
            assert save(proc, 0)["keyStoredIn"] == "keychain"
        armed.write_text(point)
        if scenario == "remove":
            request(proc, "model/remove", dict(id="fixture"), False)
        else:
            save(proc, 1, False)
        deadline = time.monotonic() + 8
        while not (root / "paused").exists():
            if select.select([proc.stdout], [], [], 0.02)[0]:
                reply = read_reply(proc)
                raise AssertionError(f"{scenario} finished before {point}, backend={reply.get('keyStoredIn')}")
            if time.monotonic() >= deadline and not (root / "paused").exists():
                raise TimeoutError(f"{scenario}: native helper did not reach {point}")
        assert journal.is_file()
        assert all(value not in journal.read_text() for value in keys)
    finally:
        proc.kill()
        proc.wait(timeout=8)
        proc.stdin.close()
        proc.stdout.close()
        if scenario != "late-store":
            armed.unlink(missing_ok=True)

    if scenario == "late-store":
        # The parent died before the native utility performed its write. A new
        # runtime can start immediately, but must leave the recovery record intact.
        proc = start()
        try:
            assert not request(proc, "model/profiles", {})["profiles"]
            assert journal.exists(), "recovery raced an in-flight credential write"
        finally:
            armed.unlink(missing_ok=True)
            stop(proc)
        deadline = time.monotonic() + 5
        while not (root / "helper-stored").exists():
            assert time.monotonic() < deadline
            time.sleep(0.01)

    # The crashing runtime and its helper are independent processes. Wait for the
    # latter to release its record before asserting the result of a completed retry.
    import fcntl
    with journal.open() as record:
        deadline = time.monotonic() + 5
        while True:
            try:
                fcntl.flock(record, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                assert time.monotonic() < deadline
                time.sleep(0.01)

    expected = {"create": None, "rotate": 0, "published": 1, "remove": None, "late-store": None}[scenario]
    for _ in range(2):
        found.unlink(missing_ok=True)
        proc = start()
        try:
            result = request(proc, "model/profiles", {})
            models = [p["model"] for p in result["profiles"]]
            assert models == ([] if expected is None else [("first", "second")[expected]])
            assert result.get("activeId") == (None if expected is None else "fixture")
            assert not journal.exists()
            assert list(json.loads((root / "items").read_text()).values()) == (
                [] if expected is None else [keys[expected]])
            assert (found.read_text() if found.exists() else None) == (
                None if expected is None else keys[expected]), "boot used a different credential"
        finally:
            stop(proc)
    proc = start()
    try:
        assert save(proc, 1)["keyStoredIn"] == "keychain"
        assert request(proc, "model/remove", dict(id="fixture"))["removed"]
        assert not json.loads((root / "items").read_text())
    finally:
        stop(proc)
    assert all(key not in output.read_text() for key in keys)
    print(f"  ✓ model/{scenario}: killed at {point}, recovered twice and retried")


def exercise_backend_return(binary, bindir, root):
    home = root / "home"
    home.mkdir(parents=True)
    offline = root / "no-helper"
    offline.mkdir()
    keys = ("fixture-native-first", "fixture-file-second", "fixture-native-third")
    journal = home / ".jeeves-model-change.json"
    fallback = home / "secrets.json"

    def rpc(path, method, **params):
        request = dict(jsonrpc="2.0", id=1, method=method, params=params)
        result = subprocess.run(
            [binary, "app-server"], cwd=home,
            env=dict(JEEVES_HOME=str(home), PATH=str(path), FIXTURE_DIR=str(root), TMPDIR=str(root)),
            input=json.dumps(request) + "\n", capture_output=True, text=True, check=True, timeout=15,
        )
        assert all(key not in result.stdout + result.stderr for key in keys)
        responses = [json.loads(line) for line in result.stdout.splitlines()]
        reply, = [response for response in responses if response.get("id") == 1]
        return reply["result"]

    def save(path, model, key):
        return rpc(path, "model/save", id="fixture", provider="openai", model=model, apiKey=key)

    assert save(bindir, "first", keys[0])["keyStoredIn"] == "keychain"
    assert save(offline, "second", keys[1])["keyStoredIn"] == "file"
    assert journal.is_file(), "keep the native cleanup record while the service is unavailable"
    assert all(key not in journal.read_text() for key in keys)
    assert list(json.loads((root / "items").read_text()).values()) == [keys[0]]
    assert list(json.loads(fallback.read_text()).values()) == [keys[1]]
    assert fallback.stat().st_mode & 0o777 == 0o600

    profiles = rpc(bindir, "model/profiles")
    assert profiles["activeId"] == "fixture"
    assert [(p["model"], p["keySource"]) for p in profiles["profiles"]] == [("second", "file")]
    assert not journal.exists() and not json.loads((root / "items").read_text())
    assert list(json.loads(fallback.read_text()).values()) == [keys[1]]
    assert save(bindir, "third", keys[2])["keyStoredIn"] == "keychain"
    rpc(bindir, "model/profiles")
    assert (root / "resolved").read_text() == keys[2]
    assert not json.loads(fallback.read_text())
    assert rpc(bindir, "model/remove", id="fixture")["removed"]
    assert not json.loads((root / "items").read_text()) and not journal.exists()
    print("  ✓ model/backend recovery: native → file → native, old credentials retired")


def main():
    if sys.platform not in ("darwin", "linux"):
        print("  model crash smoke requires a command-based fixture keychain (macOS or Linux)")
        return
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="jeeves-model-") as temp:
        root = Path(temp)
        bindir = root / "bin"
        bindir.mkdir()
        helper = bindir / ("security" if sys.platform == "darwin" else "secret-tool")
        helper.write_text("#!" + sys.executable + "\n" + Path(__file__).read_text())
        helper.chmod(0o700)
        for scenario in ("late-store", "create", "rotate", "published", "remove"):
            exercise(binary, bindir, root / scenario, scenario)
        exercise_backend_return(binary, bindir, root / "backend-return")


if __name__ == "__main__":
    if Path(sys.argv[0]).name in ("security", "secret-tool"):
        sys.exit(fixture_helper())
    main()
