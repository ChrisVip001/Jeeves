#!/usr/bin/env bash
# End-to-end smoke test for app-server: really start a jeeves app-server, feed it JSON-RPC from
# stdin, and check every kind of reply. Closer to real usage than unit tests — unit tests prove
# the state is right; this proves the "protocol + transport + state" chain works.
#
#   ./scripts/smoke.sh
#
# Requires prior Rust and dashboard builds; the hub validates its UI assets.

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

need_bin

cd "$WORK"
export JEEVES_HOME="$WORK/.jeeves"

# Ask several things in one session: write → retrieve → forget → config → unimplemented →
# unknown method → bad JSON. Note: no API key here, so turn/start should report PROVIDER_ERROR,
# with a message telling the user what to do — while all other methods must keep working.
OUTPUT=$(printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"memory/remember","params":{"subject":"Alex","content":"Alex likes morning meetings"}}' \
  '{"jsonrpc":"2.0","id":2,"method":"memory/search","params":{"query":"morning"}}' \
  '{"jsonrpc":"2.0","id":3,"method":"memory/forget","params":{"subject":"Alex"}}' \
  '{"jsonrpc":"2.0","id":4,"method":"config/read","params":{}}' \
  '{"jsonrpc":"2.0","id":5,"method":"turn/start","params":{"message":"hi"}}' \
  '{"jsonrpc":"2.0","id":6,"method":"nope","params":{}}' \
  '{ bad JSON' \
  | "$BIN" app-server)

echo "$OUTPUT" | sed 's/^/  /'

# ---- Assertions ----
fail() { echo "✗ $1" >&2; exit 1; }

echo "$OUTPUT" | grep -q '"id":1,"result":{"fact"'          || fail "memory/remember wrote nothing"
echo "$OUTPUT" | grep -q '"id":2,"result":{"episodes":\[\],"facts":\[{"content":"Alex likes morning meetings"' \
                                                              || fail "the search found nothing"
echo "$OUTPUT" | grep -q '"id":3,"result":{"removed":1}'     || fail "memory/forget deleted nothing"
echo "$OUTPUT" | grep -q '"id":4,"result":{"config":'        || fail "config/read returned no config"
echo "$OUTPUT" | grep -q '"id":5,"error":{"code":-32000'     || fail "with no key, turn/start should report PROVIDER_ERROR"
echo "$OUTPUT" | grep -qE 'no API key'      || fail "PROVIDER_ERROR should say what to do"
echo "$OUTPUT" | grep -q '"id":6,"error":{"code":-32601'     || fail "an unknown method should report METHOD_NOT_FOUND"
echo "$OUTPUT" | grep -q '"code":-32700'                     || fail "bad JSON should report PARSE_ERROR"

# Memory must really be an openable file — that's the whole point of "local-first".
[[ -f "$WORK/.jeeves/state.db" ]] || fail "state.db was not created under .jeeves/"
if command -v sqlite3 >/dev/null; then
  sqlite3 "$JEEVES_HOME/state.db" '.tables' >/dev/null || fail "sqlite3 could not read state.db"
  echo "  ✓ state.db opens directly in sqlite3"
else
  echo "  ⚠ sqlite3 is not installed; skipping the direct-open check"
fi

# Start a fake SSE endpoint and print its port. The two sections below each need a fresh port
# (it exits after serving one streaming reply).
start_fake() {
  local port_file="$1"
  python3 "$REPO/scripts/fake_sse_server.py" >"$port_file" &
  FAKE_PID=$!
track_pid "$FAKE_PID"
  for _ in $(seq 1 50); do
    [[ -s "$port_file" ]] && break
    sleep 0.1
  done
  head -n1 "$port_file" 2>/dev/null || true
}

# ---- Streaming: connect to the local fake SSE endpoint, verify SSE → textDelta → reply ----
# No real key: model/save points a profile at the fake endpoint (keyEnv pointer); requests go out.
PORT="$(start_fake "$WORK/fake_port")"
[[ -n "$PORT" ]] || fail "the fake SSE endpoint did not come up"

# Configure the model via the protocol itself (model/save): --env pointer key + test env var —
# no real keychain, no plaintext file; base_url points at the fake endpoint. Also runs that path.
STREAM_OUT=$(printf '%s\n' \
  "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"model/save\",\"params\":{\"id\":\"fake\",\"provider\":\"anthropic\",\"apiKey\":null,\"keyEnv\":\"JEEVES_FAKE_KEY\",\"baseUrl\":\"http://127.0.0.1:$PORT\"}}" \
  '{"jsonrpc":"2.0","id":2,"method":"turn/start","params":{"message":"hello"}}' \
  | JEEVES_FAKE_KEY=test JEEVES_HOME="$WORK/.jeeves-stream" "$BIN" app-server)

echo "$STREAM_OUT" | sed 's/^/  [stream] /'

echo "$STREAM_OUT" | grep -q '"method":"turn/notification"' || fail "streaming sent no notification"
echo "$STREAM_OUT" | grep -q '"textDelta"'                 || fail "no textDelta notification"
echo "$STREAM_OUT" | grep -q '"delta":"hello"'              || fail "the first delta is wrong"
echo "$STREAM_OUT" | grep -q '"reply":"hello, world"'         || fail "the final reply is not the joined text"
echo "$STREAM_OUT" | grep -q '"id":2,"result":{"turnId"'   || fail "turn/start gave no proper reply"
echo "  ✓ streaming: SSE → textDelta → the full reply"

# Exercise explicit terminal reads against an unavailable store in this disposable home.
python3 - "$JEEVES_HOME/state.db" <<'PY_DB'
import sqlite3, sys
with sqlite3.connect(sys.argv[1]) as db:
    db.execute("DROP VIEW active_facts")
    db.execute("INSERT INTO episodes(happened_at, summary) VALUES('2026-01-02', 'fixture episode still available')")
    db.execute("ALTER TABLE sessions RENAME TO fixture_unavailable_sessions")
PY_DB
READS="$(printf '%s\n' '/memory' '/sessions' '/quit' | "$BIN" 2>&1)"
grep -q 'Could not read memory' <<<"$READS" || fail "/memory hid a storage failure"
grep -q 'fixture episode still available' <<<"$READS" || fail "/memory suppressed the healthy episode index"
grep -q 'Could not list sessions' <<<"$READS" || fail "/sessions hid a storage failure"
grep -q 'Bye.' <<<"$READS" || fail "a failed read should leave the terminal usable"

python3 "$REPO/scripts/smoke-model-credentials.py" "$BIN"

echo "✓ smoke test passed"
