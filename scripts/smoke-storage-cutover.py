#!/usr/bin/env python3
"""Rehearse the incremental-to-baseline cutover against consistent, private SQLite copies."""
import argparse
import hashlib
import json
import os
import sqlite3
import subprocess
import tempfile
from contextlib import closing
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TABLES = (
    "agent_tasks", "task_artifacts", "task_runs", "task_sources", "task_completions",
    "task_verifications", "approval_log", "interaction_receipts", "desktop_receipts",
    "receipts", "tool_operations", "usage_ledger",
)


def snapshot(source, dest):
    with closing(sqlite3.connect(source.resolve().as_uri() + "?mode=ro", uri=True)) as live:
        with closing(sqlite3.connect(dest)) as copy:
            live.backup(copy)


def quoted(identifier):
    return '"' + identifier.replace('"', '""') + '"'


def fingerprint(db, table, columns):
    digest = hashlib.sha256()
    count = 0
    names = ",".join(map(quoted, columns))
    for row in db.execute(f"SELECT {names} FROM {quoted(table)} ORDER BY {names}"):
        for field in row:
            value = field if isinstance(field, bytes) else repr(field).encode()
            digest.update(type(field).__name__.encode() + b":" + len(value).to_bytes(8, "big") + value)
        count += 1
    return count, digest.digest()


def compare(before, after, old, new):
    columns = [r[1] for r in before.execute(f"PRAGMA table_info({quoted(old)})")]
    assert columns, old
    original = fingerprint(before, old, columns)
    assert original == fingerprint(after, new, columns), f"contents differ in {new}"
    print(f"{new}: {original[0]} rows identical")


def run(state, accounts):
    with tempfile.TemporaryDirectory(prefix="jeeves-cutover-") as directory:
        root = Path(directory)
        bundle, home = root / "bundle", root / "home"
        bundle.mkdir()
        snapshot(state, bundle / "state.db")
        snapshot(accounts, root / "accounts.sqlite")
        env = dict(os.environ, JEEVES_HOME=str(home))
        binary = str(ROOT / "target/debug/jeeves")
        with closing(sqlite3.connect(bundle / "state.db")) as source:
            version = source.execute("SELECT max(version) FROM _sqlx_migrations").fetchone()[0]
            assert version in (24, 25), "requires an incremental v24 or v25 snapshot"
            (bundle / "jeeves-home.json").write_text(json.dumps({"format": 1, "schema": version}))
            subprocess.run([binary, "home", "import", str(bundle)], env=env, check=True)
            backup, = home.glob("state.db.pre-*")
            with closing(sqlite3.connect(backup)) as preserved:
                compare(source, preserved, "_sqlx_migrations", "_sqlx_migrations")
            subprocess.run([binary, "home", "export", str(root / "reopened")], env=env, check=True)
            with closing(sqlite3.connect(root / "reopened/state.db")) as after:
                assert after.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
                assert not after.execute("PRAGMA foreign_key_check").fetchall()
                for table in TABLES:
                    compare(source, after, table, "historical_turn_usage" if table == "usage_ledger" else table)
                compare(source, after, "_sqlx_migrations", "historical_schema_migrations")
                if version == 24:
                    assert source.execute("SELECT id,forked_from FROM sessions WHERE forked_from IS NOT NULL ORDER BY id").fetchall() == after.execute("SELECT id,forked_from FROM historical_session_origins ORDER BY id").fetchall()
                assert after.execute("SELECT count(*) FROM model_requests").fetchone() == (0,)
                assert after.execute("SELECT count(*) FROM tool_operations WHERE output_preview IS NOT NULL").fetchone() == (0,)
                unknown = after.execute("SELECT count(*) FROM tool_operations WHERE status IN ('succeeded','failed') AND output_preview IS NULL").fetchone()[0]
                print(f"{unknown} legacy outcomes lack executor previews; migration created no evidence")
        with closing(sqlite3.connect(root / "accounts.sqlite")) as copied:
            with closing(sqlite3.connect(accounts.resolve().as_uri() + "?mode=ro", uri=True)) as live:
                compare(copied, live, "gw_users", "gw_users")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("state", type=Path)
    parser.add_argument("accounts", type=Path)
    args = parser.parse_args()
    run(args.state, args.accounts)
