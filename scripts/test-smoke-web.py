#!/usr/bin/env python3
"""Regression controls for the offline Web smoke watchdog and turn correlation."""
import json
from pathlib import Path
import runpy
import socket
import unittest

smoke = runpy.run_path(str(Path(__file__).with_name("smoke-web.py")))


def frame(kind, turn, session=None):
    event = dict(type=kind, turnId=turn, sessionId=session)
    return b"data: " + json.dumps(event).encode() + b"\n"


class SmokeNotifications(unittest.TestCase):
    def test_only_the_launched_turn_can_request_approval_or_complete(self):
        frames = [
            b": keepalive\n",
            frame("turnStarted", "other", "other-session"),
            frame("approvalRequested", "other"),
            frame("turnCompleted", "other"),
            frame("turnStarted", "expected", "fixture-session"),
            frame("approvalRequested", "other"),
            frame("approvalRequested", "expected"),
            frame("turnCompleted", "expected"),
        ]
        events = list(smoke["matched_events"](frames, "fixture-session"))
        self.assertEqual([event["turnId"] for event in events], ["expected"] * 3)
        self.assertEqual([event["type"] for event in events],
                         ["turnStarted", "approvalRequested", "turnCompleted"])

    def test_heartbeats_then_a_blocked_read_cannot_extend_the_deadline(self):
        reader, writer = socket.socketpair()
        with reader, writer, reader.makefile("rb") as stream:
            # Leave the connection open so reading blocks after the heartbeats.
            writer.sendall(b": keepalive\n" * 3 + frame("turnCompleted", "other"))
            reader.settimeout(1)
            with self.assertRaises(smoke["NotificationDeadline"]):
                with smoke["notification_deadline"](.1):
                    list(smoke["matched_events"](stream, "fixture-session"))


if __name__ == "__main__":
    unittest.main()
