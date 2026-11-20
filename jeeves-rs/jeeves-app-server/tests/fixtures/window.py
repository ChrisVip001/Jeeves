#!/usr/bin/env python3
"""Protocol fixture with injected native failures, without access to window APIs."""
import base64
import json
import sys
import time
import uuid
from pathlib import Path

root = Path(__file__).parent
for line in sys.stdin:
    cmd = json.loads(line)
    mode = (root / "mode").read_text()
    if mode == "choosing" and cmd["kind"] == "select":
        (root / "dialog").touch()
        while (root / "mode").read_text() == "choosing":
            time.sleep(0.01)
    if mode == "deny":
        reply = {"kind": "denied", "message": "Accessibility permission denied"}
    elif mode == "switch":
        reply = {"kind": "reselect", "message": "Selected window changed"}
    elif cmd["kind"] == "frame":
        header = bytes.fromhex("ffd8ffc0000b080001000101011100ffd9")
        reply = {"kind": "frame", "jpeg": base64.b64encode(header).decode()}
    else:
        if cmd["kind"] == "act":
            with (root / "effects").open("a") as effects:
                effects.write(cmd["action"] + "\n")
            if mode == "block":
                time.sleep(180)
        reply = {"kind": "observed", "observation": {
            "observationId": str(uuid.uuid4()), "window": "Fixture",
            "elements": [{"id": 1, "label": "Field", "actions": ["setText"]}]
        }}
    print(json.dumps(reply), flush=True)
