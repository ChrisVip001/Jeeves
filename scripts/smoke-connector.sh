#!/usr/bin/env bash
# End-to-end smoke for the Feishu connector: a real `jeeves connector feishu` against a
# scripted Feishu server.
#
# What only this can prove: the CLI path the human uses, the allowlist boundary as the
# user experiences it, and the request a real server would receive. Rust tests cover
# the same seams in-process; this one runs the shipped binary.
#
#   ./scripts/smoke-connector.sh
#
# Requires: a cargo build, python3. No network, no real Feishu tenant.

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

fail() { echo "✗ $1" >&2; exit 1; }

need_bin

mkdir -p "$WORK/.jeeves"

# ---- 1. No config: nothing is enabled, and the CLI says so --------------------------
OUT="$(JEEVES_HOME="$WORK/.jeeves" "$BIN" connector list 2>&1)" || fail "connector list failed: $OUT"
grep -q "No connectors yet" <<<"$OUT" || fail "with no config it has to say so: $OUT"
echo "  ✓ no config, no connector enabled"

# ---- 2. Feishu: allowlist, draft, then one live message ---------------------------
python3 "$REPO/scripts/fake_feishu_server.py" "$WORK/feishu.log" >"$WORK/feishu_port" &
FEISHU_PID=$!
track_pid "$FEISHU_PID"
for _ in $(seq 1 50); do
  [[ -s "$WORK/feishu_port" ]] && break
  sleep 0.1
done
FEISHU_PORT="$(head -n1 "$WORK/feishu_port" 2>/dev/null || true)"
[[ -n "$FEISHU_PORT" ]] || fail "the fake Feishu endpoint did not come up"

python3 - "$WORK/.jeeves/connectors.json" "$FEISHU_PORT" <<'PY'
import json, os, sys
path, port = sys.argv[1], sys.argv[2]
config = json.load(open(path)) if os.path.exists(path) else {}
config["feishu"] = {
    "app_id": "cli_smoke",
    "app_secret": "env:JEEVES_FEISHU_SMOKE_SECRET",
    "receive_id_type": "open_id",
    "allow_recipients": ["ou_alice"],
    "live": True,
    "base_url": f"http://127.0.0.1:{port}",
}
json.dump(config, open(path, "w"))
PY

set +e
OUT="$(JEEVES_HOME="$WORK/.jeeves" "$BIN" connector feishu --to ou_mallory --text hi 2>&1)"
CODE=$?
set -e
[[ $CODE -ne 0 ]] || fail "outside the Feishu allowlist this should fail"
grep -q "ou_mallory" <<<"$OUT" || fail "the Feishu error has to name the recipient: $OUT"
echo "  ✓ outside the Feishu allowlist: refused"

OUT="$(JEEVES_HOME="$WORK/.jeeves" "$BIN" connector feishu --to ou_alice --text "hold this as a draft" --dry-run 2>&1)"
grep -q "Draft" <<<"$OUT" || fail "a dry run should write a draft: $OUT"
ls "$WORK/.jeeves/outbox" | grep -q "feishu-" || fail "there is no Feishu draft in the outbox"
echo "  ✓ Feishu dry run: writes the outbox only"

JEEVES_FEISHU_SMOKE_SECRET=s3cret JEEVES_HOME="$WORK/.jeeves" \
  "$BIN" connector feishu --to ou_alice --text "a smoke-test message" >"$WORK/feishu_out.txt" 2>&1 \
  || fail "the live Feishu send failed: $(cat "$WORK/feishu_out.txt")"
grep -q "om_fake" "$WORK/feishu_out.txt" || fail "it should report a message_id: $(cat "$WORK/feishu_out.txt")"
[[ -s "$WORK/feishu.log" ]] || fail "the fake Feishu received no message request"

python3 - "$WORK/feishu.log" <<'PY' || fail "the Feishu request body is wrong"
import json, sys
line = open(sys.argv[1], encoding="utf-8").read().strip().splitlines()[0]
request = json.loads(line)
assert "receive_id_type=open_id" in request["path"], request["path"]
assert request["auth"] == "Bearer t-fake", request["auth"]
assert request["body"]["receive_id"] == "ou_alice", request["body"]
assert request["body"]["msg_type"] == "text", request["body"]
content = json.loads(request["body"]["content"])
assert content["text"] == "a smoke-test message", content
PY
echo "  ✓ Feishu live: tenant token → text message, with receive_id and content shaped right"

# ---- 3. A user token refreshed in the background: nobody sends a message, it renews itself ---
# The chain this proves: connectors.json says as:user → app-server starts the renewer →
# the renewer sees a token inside its lead window → refresh grant over real HTTP → the file
# is rewritten with the rotated pair.
RENEW_HOME="$WORK/renew"
mkdir -p "$RENEW_HOME/connector-auth"
python3 "$REPO/scripts/fake_feishu_server.py" "$WORK/renew.log" >"$WORK/renew_port" &
RENEW_PID=$!
track_pid "$RENEW_PID"
for _ in $(seq 1 50); do
  [[ -s "$WORK/renew_port" ]] && break
  sleep 0.1
done
RENEW_PORT="$(head -n1 "$WORK/renew_port" 2>/dev/null || true)"
[[ -n "$RENEW_PORT" ]] || fail "the fake Feishu endpoint used for the refresh did not come up"

cat > "$RENEW_HOME/connectors.json" <<EOF_RENEW
{"feishu": {"app_id": "cli_fake", "app_secret": "env:JEEVES_FEISHU_SMOKE_SECRET",
            "as": "user", "allow_recipients": ["ou_alice"], "live": false,
            "base_url": "http://127.0.0.1:$RENEW_PORT"}}
EOF_RENEW

# Five seconds of life left: the renewer's first glance already lands inside the lead window.
python3 - "$RENEW_HOME/connector-auth/feishu.json" <<'PY_TOKEN'
import json, sys, time
json.dump({"access_token": "u-old", "refresh_token": "r-old",
           "expires_at": int(time.time()) + 5}, open(sys.argv[1], "w"))
PY_TOKEN

# stdin stays open for a few seconds: app-server shuts down when the pipe closes, and the
# renewer lives inside it.
( printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"session/list","params":{}}'; sleep 8 ) \
  | JEEVES_HOME="$RENEW_HOME" JEEVES_PROVIDER=anthropic JEEVES_API_KEY=test \
      JEEVES_FEISHU_SMOKE_SECRET=s3cret \
      "$BIN" app-server >"$WORK/renew_out.log" 2>&1 &
RENEW_SERVER=$!
track_pid "$RENEW_SERVER"

RENEWED=""
for _ in $(seq 1 100); do
  RENEWED="$(python3 - "$RENEW_HOME/connector-auth/feishu.json" <<'PY_READ'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get("access_token", ""))
except Exception:
    print("")
PY_READ
)"
  [[ "$RENEWED" == "u-renewed" ]] && break
  sleep 0.1
done
kill "$RENEW_SERVER" 2>/dev/null || true
kill "$RENEW_PID" 2>/dev/null || true

[[ "$RENEWED" == "u-renewed" ]] || { cat "$WORK/renew_out.log"; fail "the token was not refreshed in the background (got: $RENEWED)"; }
grep -q "refreshed in the background" "$WORK/renew_out.log" || { cat "$WORK/renew_out.log"; fail "the refresh left no word in the log"; }

python3 - "$WORK/renew.log" <<'PY_GRANT' || fail "the refresh request is shaped wrong"
import json, sys
lines = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
grant = next(item for item in lines if "oauth/token" in item["path"])
assert grant["body"]["grant_type"] == "refresh_token", grant
assert grant["body"]["refresh_token"] == "r-old", grant
assert grant["body"]["client_id"] == "cli_fake", grant
PY_GRANT

python3 - "$RENEW_HOME/connector-auth/feishu.json" <<'PY_ROTATED' || fail "the rotated refresh token was not stored"
import json, sys
token = json.load(open(sys.argv[1]))
assert token["refresh_token"] == "r-renewed", token
assert token["expires_at"] > 0, token
PY_ROTATED
echo "  ✓ background refresh: with nobody sending a message the refresh grant really ran, and both the token and the rotated refresh token reached the disk"

echo "✓ connector smoke test passed"
