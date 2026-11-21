#!/usr/bin/env python3
"""Start the private Rust Runtime with a deterministic provider for cloud smoke tests."""
import importlib.util
import signal
import socket
import tempfile
import time
from pathlib import Path

spec = importlib.util.spec_from_file_location("web_fixture", Path(__file__).with_name("smoke-web.py"))
web = importlib.util.module_from_spec(spec)
spec.loader.exec_module(web)

with tempfile.TemporaryDirectory(prefix="jeeves-cloud-runtime-") as home:
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    provider, runtime = web.start(home, port)
    origin = f"http://127.0.0.1:{port}"
    signal.signal(signal.SIGTERM, lambda *_: runtime.terminate())
    try:
        for _ in range(100):
            if runtime.poll() is not None:
                raise RuntimeError(Path(home, "server.log").read_text())
            try:
                web.request(origin, "/healthz")
                break
            except OSError:
                time.sleep(.1)
        else:
            raise RuntimeError("fixture Runtime did not become ready")
        print(origin, flush=True)
        runtime.wait()
    finally:
        if runtime.poll() is None:
            runtime.terminate()
            runtime.wait(timeout=30)
        provider.shutdown()
        provider.server_close()
