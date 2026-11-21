#!/usr/bin/env bash
# End-to-end smoke test for the dashboard: really start `jeeves dashboard`, curl as the browser.
#
# The frontend's own unit tests can't prove this chain — what it really proves is the translation
# layer: four read methods → one JSON screen, turn/start → SSE, spoken messages read back. So
# instead of stubs, start a real app-server (forked by `jeeves dashboard`) and use
# scripts/fake_sse_server.py as provider: "model stream → notifications → SSE" runs end to end.
#
#   ./scripts/smoke-dashboard.sh
#
# Requires: a cargo build, a built frontend (just dashboard-build), python3.

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

ASSETS="$REPO/jeeves-ts/packages/dashboard/dist"

fail() { echo "✗ $1" >&2; exit 1; }

need_bin
[[ -f "$ASSETS/index.html" ]] || fail "the front end has not been built; run: just dashboard-build"

# Fake SSE endpoint: completes the "SSE → full reply" chain without a real key.
# The main conversation makes one streaming model request.
python3 "$REPO/scripts/fake_sse_server.py" 1 >"$WORK/fake_port" &
FAKE_PID=$!
track_pid "$FAKE_PID"
for _ in $(seq 1 50); do
  [[ -s "$WORK/fake_port" ]] && break
  sleep 0.1
done
MODEL_PORT="$(head -n1 "$WORK/fake_port" 2>/dev/null || true)"
[[ -n "$MODEL_PORT" ]] || fail "the fake SSE endpoint did not come up"

# Pick a free port for the dashboard. It scans too, but after scanning the port is hard to guess —
# so read where it actually landed straight from the startup line.
PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"

# Models and keys no longer travel via env vars (see 0.7.0): seed a models.json whose
# key_source reads JEEVES_API_KEY from the environment and whose base_url is the fake SSE
# endpoint. JEEVES_PROVIDER/JEEVES_API_KEY below stay — provider is still the fallback vendor.
mkdir -p "$WORK/.jeeves"

# One connector, draft mode: the panel must show it without the tool ever sending anything.
cat > "$WORK/.jeeves/connectors.json" <<'EOF_CONNECTOR'
{"feishu": {
  "app_id": "cli_smoke", "app_secret": "env:JEEVES_FEISHU_SMOKE_SECRET",
  "allow_recipients": ["ou_alice"],
  "live": false
}}
EOF_CONNECTOR

# Capability-center fixtures: one user skill + one built-in skill pack (JEEVES_SKILL_DIRS) + one
# built-in MCP config (JEEVES_MCP_CONFIG). The built-ins are greyed out in the page — the server's
# fence is "every write is refused", checked one by one below. The unreachable server inside the
mkdir -p "$WORK/.jeeves/skills/my-skill" "$WORK/pack/skills/builtin-skill"
cat > "$WORK/.jeeves/skills/my-skill/SKILL.md" <<'EOF_SKILL'
---
name: my-skill
description: a skill the human installed themselves
---
body
EOF_SKILL
cat > "$WORK/pack/skills/builtin-skill/SKILL.md" <<'EOF_SKILL'
---
name: builtin-skill
description: ships with the install, read-only in the UI
---
body
EOF_SKILL
cat > "$WORK/pack/mcp.json" <<'EOF_MCP'
{"servers": [{"name": "pack-server", "url": "https://pack.example/mcp"}]}
EOF_MCP
cat > "$WORK/.jeeves/models.json" <<EOF
{"active":"smoke","profiles":[{"id":"smoke","provider":"anthropic","base_url":"http://127.0.0.1:$MODEL_PORT","key_source":"env:JEEVES_API_KEY"}]}
EOF

JEEVES_PORT="$PORT" \
JEEVES_HOME="$WORK/.jeeves" \
JEEVES_SKILL_DIRS="$WORK/pack/skills" \
JEEVES_MCP_CONFIG="$WORK/pack/mcp.json" \
JEEVES_PROVIDER=anthropic JEEVES_API_KEY=test JEEVES_BASE_URL="http://127.0.0.1:$MODEL_PORT" \
  "$BIN" dashboard >"$WORK/dash.log" 2>&1 &
DASH_PID=$!
track_pid "$DASH_PID"

BASE=""
for _ in $(seq 1 100); do
  # The startup line prints the URL it actually landed on (localhost or 127.0.0.1).
  BASE="$(grep -oE '(localhost|127\.0\.0\.1):[0-9]+' "$WORK/dash.log" | head -n1 || true)"
  [[ -n "$BASE" ]] && break
  kill -0 "$DASH_PID" 2>/dev/null || { cat "$WORK/dash.log"; fail "the cockpit exited before it came up"; }
  sleep 0.1
done
[[ -n "$BASE" ]] || { cat "$WORK/dash.log"; fail "no startup line ever appeared"; }
BASE="http://$BASE"

# ---- Static files ---------------------------------------------------------------
HEAD="$(curl -sS -D - -o "$WORK/index.html" "$BASE/")"
grep -qi '^HTTP/1.1 200' <<<"$HEAD"   || fail "the front page is not 200"
grep -qi 'content-type: text/html' <<<"$HEAD" || fail "the front page is not HTML"
grep -qi 'cache-control: no-store' <<<"$HEAD" || fail "no no-cache — a changed page would not show up"
grep -q 'main.js' "$WORK/index.html"  || fail "the front page does not pull in main.js"

# The frontend was really built: that bundle comes from Vite — all three endpoints must be in
# it (they live in api.ts, proving the module was actually bundled, not an empty shell).
curl -sS -o "$WORK/main.js" -w '%{http_code}' "$BASE/main.js" | grep -q '^200$' \
  || fail "main.js cannot be fetched"

# The stylesheet must keep the **global** `[hidden]` rule. `display: flex` beats the UA's
# `[hidden] { display: none }`, and two components in this file set `display: flex` on elements
# that are toggled with `hidden`. When the rule went missing, the memory center's three views all
# rendered at once and every segment appeared to show the ledger — the data was fine, the picture
# was not. A CSS assertion is a blunt instrument, but this failure mode is invisible to every
# other check here.
curl -sS -o "$WORK/style.css" -w '%{http_code}' "$BASE/style.css" | grep -q '^200$' \
  || fail "style.css cannot be fetched"
grep -qEz '\[hidden\][[:space:]]*\{[[:space:]]*display:[[:space:]]*none[[:space:]]*!important' "$WORK/style.css" \
  || fail "style.css has no global [hidden] rule — views toggled with hidden would all show at once"

# The page names its bundle with a build stamp. index.html is network-first and main.js is
# cache-first, so without the stamp a service-worker cache pairs a new page with an old bundle —
# the page then misses ids the bundle grabs and goes blank (it happened: the whole screen, session
# list included, disappeared). The stamp changes the cache key every build, so the pair can only
# come from the same one.
BUNDLE="$(grep -oE 'main\.js\?v=[a-z0-9]+' "$WORK/index.html" | head -1)"
[[ -n "$BUNDLE" ]] || fail "the front page gives main.js no build stamp — an old cache would pair the old bundle with the new page"
curl -sS -o /dev/null -w '%{http_code}' "$BASE/$BUNDLE" | grep -q '^200$' \
  || fail "the stamped $BUNDLE cannot be fetched (the build did not stamp it, or static serving ignores the query)"
grep -q '/api/data' "$WORK/main.js" || fail "main.js has no /api/data"
# This one matches by path only: a template string with interpolation — backticks after bundling.
grep -q '/api/session' "$WORK/main.js" || fail "main.js has no /api/session"
grep -q '/api/session/main' "$WORK/main.js" || fail "main.js has no main conversation endpoint"
grep -q '/api/session/meta' "$WORK/main.js" || fail "main.js has no /api/session/meta"
grep -q '/api/turn/steer' "$WORK/main.js" || fail "main.js has no /api/turn/steer"
grep -q '/api/turn/follow-up' "$WORK/main.js" || fail "main.js has no /api/turn/follow-up"
grep -q '/api/turn' "$WORK/main.js" || fail "main.js has no /api/turn"
# The capability center's six endpoints have to be in the bundle too (one missing = that
grep -q '/api/skills' "$WORK/main.js" || fail "main.js has no /api/skills"
grep -q '/api/skills/install' "$WORK/main.js" || fail "main.js has no /api/skills/install"
grep -q '/api/skills/remove' "$WORK/main.js" || fail "main.js has no /api/skills/remove"
grep -q '/api/mcp' "$WORK/main.js" || fail "main.js has no /api/mcp"
grep -q '/api/mcp/save' "$WORK/main.js" || fail "main.js has no /api/mcp/save"
grep -q '/api/mcp/remove' "$WORK/main.js" || fail "main.js has no /api/mcp/remove"
# The memory center's six read-only endpoints plus one manual compaction: one missing = that
grep -q '/api/ledger/turns' "$WORK/main.js" || fail "main.js has no /api/ledger/turns"
grep -q '/api/ledger/turn' "$WORK/main.js" || fail "main.js has no /api/ledger/turn"
grep -q '/api/memory/index' "$WORK/main.js" || fail "main.js has no /api/memory/index"
grep -q '/api/memory/page' "$WORK/main.js" || fail "main.js has no /api/memory/page"
grep -q '/api/context' "$WORK/main.js" || fail "main.js has no /api/context"
grep -q '/api/skills/stats' "$WORK/main.js" || fail "main.js has no /api/skills/stats"
grep -q '/api/compact' "$WORK/main.js" || fail "main.js has no /api/compact"
# A refresh has to land back in the same session: if this string disappears, someone deleted
# "remember the current session" in passing, and no Rust test would go red — every refresh
grep -q 'jeeves.session' "$WORK/main.js" || fail "main.js has no jeeves.session (a refresh would lose the current session)"
grep -q '/api/schedules/jobs' "$WORK/main.js" || fail "main.js has no /api/schedules/jobs"
grep -q '/api/schedules/save' "$WORK/main.js" || fail "main.js has no /api/schedules/save"
grep -q '/api/schedules/remove' "$WORK/main.js" || fail "main.js has no /api/schedules/remove"
# Minification changes function names, but all three legacy scopes still need visible labels.
grep -q '只允许这次' "$WORK/main.js" || fail "main.js has no allow-once label"
grep -q '本会话总是允许' "$WORK/main.js" || fail "main.js has no session approval label"
grep -q '记住这条' "$WORK/main.js" || fail "main.js has no remembered approval label"
# The output of a long command has to be **visible while it runs**: a toolDelta notification
# has to reach the timeline row (P0-3). Without it the tool row only appears once the whole
grep -q 'toolDelta' "$WORK/main.js" || fail "main.js does not handle toolDelta (long output would not appear piece by piece)"

# Every id the JS grabs must exist in the page — one missing means a blank screen. That error only
# shows in the browser console, invisible to curl, so pin it here. (`must("x")` is the helper in
# main.ts that "blows up on the spot"; both files are freshly built artifacts.)
# Read source bindings, not minified local variable names.
python3 - "$REPO/jeeves-ts/packages/dashboard/src/workspace.ts" "$WORK/want" <<'PY_IDS'
from pathlib import Path
import re, sys
wanted = sorted(set(re.findall(r'must(?:<[^>]+>)?\("([a-z-]+)"\)', Path(sys.argv[1]).read_text())))
assert wanted, "No workspace DOM bindings found"
Path(sys.argv[2]).write_text("\n".join(wanted) + "\n")
PY_IDS
grep -oE 'id="[a-z-]+"' "$WORK/index.html" | grep -oE '"[a-z-]+"' | tr -d '"' | sort -u >"$WORK/have"
[[ -s "$WORK/want" ]] || fail "could not work out which ids to grab from main.js — this assertion has stopped working; do not let it pass silently"
MISSING="$(comm -23 "$WORK/want" "$WORK/have")"
[[ -z "$MISSING" ]] || fail "the page is missing these ids: $(tr '\n' ' ' <<<"$MISSING") (the browser would show a blank screen)"

# Path traversal must be blocked (ServeDir is used, not hand-built paths — but this assertion is
# worth keeping: if anyone hand-writes a static file server someday, it goes red immediately).
curl -sS -o /dev/null -w '%{http_code}' --path-as-is "$BASE/../Cargo.toml" | grep -q '^404$' \
  || fail "path traversal was not blocked"
echo "  ✓ static files + no-cache + traversal blocked"

# ---- First-screen data ---------------------------------------------------------------
curl -sS "$BASE/api/data" >"$WORK/data1.json"
grep -q '"generatedAt"' "$WORK/data1.json" || fail "the first-screen data has no generatedAt"
grep -q '"config":{"' "$WORK/data1.json"    || fail "the first-screen data has no config"
grep -q '"sessions":\[\]' "$WORK/data1.json" || fail "a fresh directory should hold no sessions"
echo "  first screen: $(head -c 160 "$WORK/data1.json")"
echo "  ✓ four read methods → one JSON screen"

# ---- Say one thing: turn/start → SSE ----------------------------------------------
curl -sS -N -X POST "$BASE/api/turn" \
  -H 'content-type: application/json' \
  -d '{"sessionId":"jeeves-main","message":"hello","stream":true}' \
  --max-time 20 >"$WORK/turn.sse" || fail "POST /api/turn did not go through"

echo "  this turn arrived as:"
sed 's/^/    /' "$WORK/turn.sse" | head -n 12

grep -q '"type":"turnStarted"'        "$WORK/turn.sse" || fail "no turnStarted notification"
grep -q '"type":"textDelta"'          "$WORK/turn.sse" || fail "no textDelta — the typewriter effect would be gone"
grep -q '"delta":"hello"'             "$WORK/turn.sse" || fail "the delta content is wrong"
grep -q '"reply":"hello, world"'      "$WORK/turn.sse" || fail "the closing full reply is wrong"
grep -q '"type":"turnCompleted"'      "$WORK/turn.sse" || fail "no turnCompleted — the front end would spin forever"
echo "  ✓ turn/start → SSE frames (the lines the browser parses)"

# ---- After speaking, the panel has content ------------------------------------------------
# That's "dashboard" vs "reading logs": the message must become something visible on screen one.
curl -sS "$BASE/api/data" >"$WORK/data2.json"
grep -q '"sessions":\[\]' "$WORK/data2.json" && fail "after one message the session list is still empty"
echo "  ✓ what was said shows up on the first screen"

# ---- One durable main conversation, with a configurable workspace ----------------------
SID=jeeves-main
curl -fsS "$BASE/api/session/main" | grep -q '"sessionId":"jeeves-main"' \
  || fail "the main conversation cannot be reopened"
curl -fsS -X POST "$BASE/api/session/update" -H 'content-type: application/json' \
  -d "{\"sessionId\":\"$SID\",\"title\":\"release check\",\"cwd\":\"$WORK\"}" \
  | grep -q '"title":"release check"' || fail "the main conversation could not be named"
curl -fsS "$BASE/api/data" | python3 -c '
import json, sys
sessions = json.load(sys.stdin)["sessions"]
assert [session["id"] for session in sessions] == ["jeeves-main"], sessions
assert sessions[0]["cwd"] == sys.argv[1], sessions
' "$WORK" || fail "the Runtime did not expose a single main conversation"
for retired in new fork; do
  status="$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/api/session/$retired" \
    -H 'content-type: application/json' -d '{"sessionId":"jeeves-main"}')"
  [[ "$status" == 404 || "$status" == 405 ]] || fail "session/$retired still responds"
done
echo "  ✓ one main conversation; session creation and forking are retired"

# ---- Spoken messages must read back ------------------------------------------------------
# That's why `/api/session` exists: after a refresh the conversation is still there, and after
# switching away and back too. The protocol gives **newest first** (easy to page earlier), and
# with only two entries here, `nextCursor` should be null — "bottomed out".
curl -sS "$BASE/api/session?sessionId=jeeves-main" >"$WORK/session.json"
grep -q '"role":"user"'      "$WORK/session.json" || fail "the history holds nothing from the user"
grep -q '"role":"assistant"' "$WORK/session.json" || fail "the history holds no assistant message"
grep -q 'hello, world'       "$WORK/session.json" || fail "the reply in the history is wrong"
grep -q '"nextCursor":null'  "$WORK/session.json" || fail "with only two entries there should be no earlier cursor"
echo "  history: $(head -c 220 "$WORK/session.json")"
echo "  ✓ what was said reads back (newest first + already at the bottom)"

# ---- Long task: create → appears on screen → pause → resume → cancel ----------------
# One door (`/api/task`) with an action field; the rules (acceptance, budget, the state
# machine) live in app-server, so the assertions check the state machine end to end.
curl -sS -X POST "$BASE/api/task" \
  -H 'content-type: application/json' \
  -d '{"action":"create","sessionId":"jeeves-main","title":"watch the server for three days","goalCondition":"watch it all three days"}' \
  >"$WORK/task1.json" || fail "creating the task did not go through"
grep -q '"status":"pending"' "$WORK/task1.json" || fail "a new task should be pending: $(cat "$WORK/task1.json")"
TASK_ID="$(sed -n 's/.*"id":"\([^"]*\)".*/\1/p' "$WORK/task1.json" | head -n1)"
[[ -n "$TASK_ID" ]] || fail "the reply carries no task id"

curl -sS "$BASE/api/data?sessionId=jeeves-main" >"$WORK/data3.json"
grep -q '"title":"watch the server for three days"' "$WORK/data3.json" || fail "the task never reached the first-screen data (the task card would be empty)"
grep -q '"goalCondition":"watch it all three days"' "$WORK/data3.json" || fail "the goal never reached the task row"
grep -q '"acceptance":\[\]' "$WORK/data3.json" || fail "with no acceptance conditions it should be an empty array"
grep -q '"spent":{"wallMs":0,"tokens":0,"tools":0}' "$WORK/data3.json" || fail "the spend has to be reported honestly (0 is a fact too)"
grep -q '"runCount":0' "$WORK/data3.json" || fail "it has never run, so runCount should be 0"
grep -q '"taskArtifacts":\[\]' "$WORK/data3.json" || fail "with no deliverables it should give an empty array"

# 0.15: a task is its own entity — the same session may hold several, and the card shows
# the newest. (It used to refuse the second one.)
curl -sS -X POST "$BASE/api/task" \
  -H 'content-type: application/json' \
  -d '{"action":"create","sessionId":"jeeves-main","title":"a second thing"}' >"$WORK/task2.json"
grep -q '"title":"a second thing"' "$WORK/task2.json" || fail "the second task should be creatable: $(cat "$WORK/task2.json")"
grep -q '"sessionId":"jeeves-main"' "$WORK/task2.json" || fail "the task row should carry the session id back"

# Acceptance conditions are normalised at creation: the card's wording and the checker's
# condition are the same sentence, so this string is a real end-to-end assertion.
curl -sS -X POST "$BASE/api/task" -H 'content-type: application/json' \
  -d "{\"action\":\"create\",\"title\":\"write a report\",\"cwd\":\"$WORK\",\"acceptance\":[{\"kind\":\"fileExists\",\"path\":\"report.md\"}]}" \
  >"$WORK/task3.json" || fail "a task with acceptance conditions did not get created"
grep -q '"label":"report.md exists"' "$WORK/task3.json" || fail "the acceptance condition has no human label: $(cat "$WORK/task3.json")"
grep -q '"status":"pending"' "$WORK/task3.json" || fail "a task with acceptance conditions should be pending too"
grep -q '"sessionId":"jeeves-main"' "$WORK/task3.json" \
  || fail "an ownerless public task must attach to the main conversation"

curl -sS -X POST "$BASE/api/task" -H 'content-type: application/json' \
  -d "{\"action\":\"suspend\",\"taskId\":\"$TASK_ID\"}" >"$WORK/suspend.json"
grep -q '"status":"suspended"' "$WORK/suspend.json" || fail "the suspend did not take effect"

curl -sS -X POST "$BASE/api/task" -H 'content-type: application/json' \
  -d "{\"action\":\"resume\",\"taskId\":\"$TASK_ID\"}" >"$WORK/resume.json"
grep -q '"status":"running"' "$WORK/resume.json" || fail "the resume did not take effect"

curl -sS -X POST "$BASE/api/task" -H 'content-type: application/json' \
  -d "{\"action\":\"cancel\",\"taskId\":\"$TASK_ID\"}" >"$WORK/cancel.json"
grep -q '"status":"cancelled"' "$WORK/cancel.json" || fail "the cancel failed"

curl -sS "$BASE/api/tasks" >"$WORK/tasks.json"
grep -q '"tasks":\[' "$WORK/tasks.json" || fail "the task list has no tasks array"
# Three tasks exist, and the list is a list of rows — not a per-session singleton.
TASK_COUNT="$(python3 -c "import json;print(len(json.load(open('$WORK/tasks.json'))['tasks']))")"
[[ "$TASK_COUNT" -ge 3 ]] || fail "the task list should hold 3 rows (it holds $TASK_COUNT)"
echo "  ✓ tasks: create → visible on the first screen (acceptance / spend / run count) → a second one works too → suspend → resume → cancel"

# The connector panel is read-only truth from connectors.json: draft mode + allowlist width.
curl -sS "$BASE/api/data?sessionId=jeeves-main" >"$WORK/data4.json"
grep -q '"connectors":\[{"id":"feishu","live":false,"allowRecipients":1}\]' "$WORK/data4.json" \
  || fail "the connector panel data is wrong: $(python3 -c "import json,sys; print(json.load(open('$WORK/data4.json')).get('connectors'))")"
echo "  ✓ connectors: draft mode + a one-entry allowlist, shown as they are"

# ---- Capability center: skills / MCP connectors -------------------------------------------
# Built-in rows are greyed out in the page; the fence that matters is the server's:
# every write refuses them — the user's own files are the only writable ones.
curl -sS "$BASE/api/skills" >"$WORK/skills.json"
python3 - "$WORK/skills.json" <<'PY' || fail "the skill list is wrong: $(cat "$WORK/skills.json")"
import json, sys
rows = json.load(open(sys.argv[1]))["skills"]
by_name = {row["name"]: row for row in rows}
assert {"my-skill", "builtin-skill"} <= set(by_name), rows
assert by_name["my-skill"]["source"] == "user", rows
assert by_name["builtin-skill"]["source"] == "builtin", rows
assert by_name["my-skill"]["allowModelInvocation"] is True, rows
assert by_name["my-skill"]["folder"].endswith("skills/my-skill"), rows
PY

# Install: paste a SKILL.md (the same validation gate as jeeves skill install)
curl -sS -X POST "$BASE/api/skills/install" -H 'content-type: application/json' \
  -d '{"text":"---\nname: pasted-skill\ndescription: pasted from the dashboard\n---\n\nbody\n"}' \
  >"$WORK/skill-install.json"
grep -q '"ok":true' "$WORK/skill-install.json" \
  || fail "pasting an install did not work: $(cat "$WORK/skill-install.json")"
curl -sS "$BASE/api/skills" | grep -q '"name":"pasted-skill"' \
  || fail "the installed skill does not show up in the list"
# A duplicate name is refused (never overwritten — a skill is instructions; read it first)
curl -sS -X POST "$BASE/api/skills/install" -H 'content-type: application/json' \
  -d '{"text":"---\nname: pasted-skill\ndescription: another one\n---\n\nbody\n"}' \
  | grep -q 'already exists' || fail "a duplicate skill name should be refused"
# Anything that is not a SKILL.md is refused
curl -sS -X POST "$BASE/api/skills/install" -H 'content-type: application/json' \
  -d '{"text":"just some text"}' | grep -q 'not a valid skill' || fail "invalid content should be refused"
# A built-in skill cannot be deleted; a user skill can
curl -sS -X POST "$BASE/api/skills/remove" -H 'content-type: application/json' \
  -d '{"name":"builtin-skill"}' | grep -q 'built-in' || fail "deleting a built-in skill should be refused"
curl -sS -X POST "$BASE/api/skills/remove" -H 'content-type: application/json' \
  -d '{"name":"pasted-skill"}' >"$WORK/skill-remove.json"
grep -q '"ok":true' "$WORK/skill-remove.json" || fail "a user skill cannot be deleted: $(cat "$WORK/skill-remove.json")"
curl -sS "$BASE/api/skills" | grep -q '"name":"pasted-skill"' && fail "the deleted skill is still in the list"

curl -sS "$BASE/api/mcp" >"$WORK/mcp.json"
python3 - "$WORK/mcp.json" <<'PY' || fail "the MCP list is wrong: $(cat "$WORK/mcp.json")"
import json, sys
rows = json.load(open(sys.argv[1]))["servers"]
assert len(rows) == 1, rows
assert rows[0]["name"] == "pack-server", rows
assert rows[0]["source"] == "builtin", rows
assert rows[0]["kind"] == "url", rows
assert rows[0]["auth"] == "none", rows
PY

# Add: lands in <home>/mcp.json; filling in both transports is refused (the same gate the
curl -sS -X POST "$BASE/api/mcp/save" -H 'content-type: application/json' \
  -d '{"name":"notes","url":"https://notes.example/mcp","authEnv":"NOTES_API_KEY"}' \
  >"$WORK/mcp-save.json"
grep -q '"ok":true' "$WORK/mcp-save.json" || fail "adding the connector did not work: $(cat "$WORK/mcp-save.json")"
grep -q 'Effective on the next turn' "$WORK/mcp-save.json" || fail "it should say when the connection changes: $(cat "$WORK/mcp-save.json")"
python3 -c "import json;d=json.load(open('$WORK/.jeeves/mcp.json'));assert [s['name'] for s in d['servers']]==['notes'],d" \
  || fail "mcp.json did not reach the disk, or holds the wrong content"
curl -sS -X POST "$BASE/api/mcp/save" -H 'content-type: application/json' \
  -d '{"name":"both","url":"https://h/mcp","command":"npx"}' | grep -q 'not both' \
  || fail "filling in both transports should be refused"
curl -sS -X POST "$BASE/api/mcp/save" -H 'content-type: application/json' \
  -d '{"name":"my server","url":"https://h/mcp"}' | grep -q 'prefix' \
  || fail "a name with a space should be refused"
# Refused when it collides with the built-in pack: at runtime the earlier one wins, so another
curl -sS -X POST "$BASE/api/mcp/save" -H 'content-type: application/json' \
  -d '{"name":"pack-server","url":"https://mine.example/mcp"}' | grep -q 'built-in' \
  || fail "a connector colliding with the built-in pack should be refused"
# The same name replaces rather than appends; stdio arguments are written as given
curl -sS -X POST "$BASE/api/mcp/save" -H 'content-type: application/json' \
  -d '{"name":"notes","url":"https://moved.example/mcp"}' | grep -q '"ok":true' \
  || fail "saving under an existing name did not work"
curl -sS -X POST "$BASE/api/mcp/save" -H 'content-type: application/json' \
  -d '{"name":"fs","command":"npx","args":["-y","server"]}' | grep -q '"ok":true' \
  || fail "the stdio connector was not added"
python3 -c "import json;d=json.load(open('$WORK/.jeeves/mcp.json'));names=[s['name'] for s in d['servers']];assert names==['notes','fs'],names" \
  || fail "the same name should replace, not append"
curl -sS "$BASE/api/mcp" >"$WORK/mcp2.json"
python3 - "$WORK/mcp2.json" <<'PY' || fail "the MCP list after saving is wrong: $(cat "$WORK/mcp2.json")"
import json, sys
rows = {row["name"]: row for row in json.load(open(sys.argv[1]))["servers"]}
assert set(rows) == {"pack-server", "notes", "fs"}, rows
assert rows["pack-server"]["source"] == "builtin", rows
assert rows["notes"]["source"] == "user", rows
assert rows["notes"]["url"] == "https://moved.example/mcp", rows
assert rows["fs"]["kind"] == "command", rows
assert rows["fs"]["args"] == ["-y", "server"], rows
PY
# A user connector can be deleted; the one in the built-in config cannot, and should not be
curl -sS -X POST "$BASE/api/mcp/remove" -H 'content-type: application/json' \
  -d '{"name":"notes"}' | grep -q '"ok":true' || fail "a user connector cannot be deleted"
curl -sS -X POST "$BASE/api/mcp/remove" -H 'content-type: application/json' \
  -d '{"name":"pack-server"}' | grep -q 'built-in' || fail "deleting a built-in connector should be refused"
curl -sS "$BASE/api/mcp" | grep -q '"name":"notes"' && fail "the deleted connector is still in the list"
curl -sS "$BASE/api/mcp" | grep -q '"name":"pack-server"' || fail "the built-in connector should still be there"
echo "  ✓ capability center: built-ins greyed out and write-refused, user skills/connectors added and removed, duplicates and invalid input both answered"

# An invalid history address cannot expose the main conversation.
curl -sS "$BASE/api/session?sessionId=nobody" | grep -q '"data":\[\]' \
  || fail "an empty session must not see what others said"
echo "  ✓ an unknown history address returns no messages"

# ---- Nothing is running now ---------------------------------------------------------------
# turn/list answers "what is happening" from the live turn table; idle must be an empty array,
# not an error and not a guess. (The non-empty side is covered in app-server's own tests.)
curl -sS "$BASE/api/turns" | grep -q '"turns":\[\]' || fail "when idle, turn/list should be an empty array"
curl -sS "$BASE/api/turns?sessionId=jeeves-main" | grep -q '"turns":\[\]' \
  || fail "filtering by session should not fail"
echo "  ✓ turn/list: an empty array when idle"

# Human operation review is available independently of model execution.
curl -sS -X POST "$BASE/api/turn-operation" -H 'content-type: application/json' \
  -d '{"action":"operations","sessionId":"jeeves-main"}' | grep -q '"operations":\[\]' \
  || fail "ordinary-chat operation inspection should return an empty list"
curl -sS -X POST "$BASE/api/turn-operation" -H 'content-type: application/json' \
  -d '{"action":"reconcile","sessionId":"jeeves-main","turnId":"missing","operationId":"missing","checkpointDigest":"stale","resolution":"not_executed","evidence":"Synthetic check"}' \
  | grep -q '"code":-32602' || fail "an unavailable chat operation must be refused"
echo "  ✓ ordinary-chat operations: inspect without a model call, invalid reconciliation refused"


# ---- 0.16: steering needs a turn that is actually running --------------------------------
# With nothing in flight this must fail **and say why** — silently starting a new turn would
# turn "add to what you're doing" into "start something else" without telling anyone.
STEER_ERR="$(curl -sS -X POST "$BASE/api/turn/steer" -H 'content-type: application/json' \
  -d '{"sessionId":"jeeves-main","message":"have a look at the README too"}')"
grep -q 'has no turn running' <<<"$STEER_ERR" || fail "when idle, turn/steer has to say there is no turn: $STEER_ERR"
echo "  ✓ turn/steer: with no turn running it says so instead of silently starting one"

# "Queue for after" also needs a turn to wait on: queueing while idle would hang a sentence
# off a turn that has already ended, which is worse than a refusal — a human would believe
FOLLOW_ERR="$(curl -sS -X POST "$BASE/api/turn/follow-up" -H 'content-type: application/json' \
  -d '{"sessionId":"jeeves-main","message":"add it to the CHANGELOG"}')"
grep -q 'has no turn running' <<<"$FOLLOW_ERR" \
  || fail "when idle, turn/follow-up has to say there is no turn: $FOLLOW_ERR"
echo "  ✓ turn/follow-up: with no turn running it does not pretend to have queued anything"

# ---- Memory center: the read-only windows really answer -----------------------------------
# Every read endpoint has to return JSON rather than 500. `/api/context` is `usage: null` when
# no turn has run — that is "not measured yet", not "zero usage"; the front end shows an empty
curl -sS "$BASE/api/ledger/turns?sessionId=jeeves-main" >"$WORK/ledger.json" \
  || fail "GET /api/ledger/turns did not go through"
grep -q '"turns"' "$WORK/ledger.json" || fail "the ledger should return turns"
grep -q '"entries":\[\]' "$WORK/ledger.json" \
  || fail "with no turn opened it should carry no entries"
curl -sS "$BASE/api/memory/index" | grep -q '"catalog"' || fail "/api/memory/index should return the catalog"
curl -sS "$BASE/api/memory/page?page=definitely-not-a-page" \
  | grep -q '"page":null' || fail "a page that does not exist should be page:null (not a pretend empty page)"
curl -sS "$BASE/api/context?sessionId=jeeves-main" | grep -q '"usage"' \
  || fail "/api/context should return usage (null is allowed)"
curl -sS "$BASE/api/skills/stats" | grep -q '"data"' || fail "/api/skills/stats should return data"
# A missing parameter has to be refused explicitly, not turned into an empty string for the query.
curl -sS "$BASE/api/ledger/turns" | grep -q 'missing parameter sessionId' \
  || fail "a missing sessionId has to say what is missing"
echo "  ✓ memory center: ledger / catalog / memory page / usage / skill hits all answer"

# ---- Capability center schedules: the declaration and its state in one table -----------------
# Creating one writes a **declaration** (a row in state.db), it does not fire anything;
# what is verified here is "written, listed, refused clearly, deleted" — firing is jeeves
curl -sS "$BASE/api/schedules/jobs" | grep -q '"jobs"' \
  || fail "/api/schedules/jobs should return jobs"
printf '%s' '{"name":"smoke-digest","cron":"0 8 * * *","prompt":"write a briefing","delivery":"outbox"}' \
  >"$WORK/sched.json"
curl -sS -X POST -H 'content-type: application/json' --data @"$WORK/sched.json" \
  "$BASE/api/schedules/save" | grep -q '"ok":true' || fail "creating a schedule failed"
curl -sS "$BASE/api/schedules/jobs" | grep -q 'smoke-digest' \
  || fail "the new schedule does not show up in the list"
# A bad cron has to be refused as a sentence, not 500 and not saved quietly.
curl -sS -X POST -H 'content-type: application/json' \
  --data '{"name":"smoke-digest","cron":"0 8 * *","prompt":"x"}' \
  "$BASE/api/schedules/save" | grep -q 'five fields' || fail "a misspelt cron has to say it needs five fields"
curl -sS -X POST -H 'content-type: application/json' --data '{"name":"smoke-digest"}' \
  "$BASE/api/schedules/remove" | grep -q '"ok":true' || fail "deleting the schedule failed"
echo "  ✓ schedules: create → visible in the list → bad cron refused → deleted"

# Two simultaneous uploads with the same client name retain both bodies in this home.
python3 - "$BASE" "$WORK/.jeeves" <<'PY_UPLOAD'
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from urllib.request import Request, urlopen
import json, sys
base, home = sys.argv[1:]
def upload(body):
    req = Request(base + "/api/upload?name=same.txt", data=body)
    with urlopen(req, timeout=10) as resp:
        return json.load(resp)["path"]
bodies = [b"first upload", b"second upload"]
with ThreadPoolExecutor(2) as workers:
    paths = list(workers.map(upload, bodies))
assert paths[0] != paths[1], paths
for name, contents in zip(paths, bodies):
    path = Path(name)
    assert path.parts[0] == "uploads" and path.suffix == ".txt", path
    assert (Path(home) / path).read_bytes() == contents
PY_UPLOAD

# An outbox on damaged storage must not be reported as an empty inbox.
if [[ -d "$WORK/.jeeves/outbox" ]]; then
  mv "$WORK/.jeeves/outbox" "$WORK/saved-outbox"
fi
touch "$WORK/.jeeves/outbox"
[[ "$(curl -sS -o "$WORK/inbox-error.json" -w '%{http_code}' "$BASE/api/inbox")" == 500 ]] \
  || fail "an unreadable outbox must return an error"
grep -q 'outbox' "$WORK/inbox-error.json" || fail "the error must identify the outbox"

echo "✓ dashboard smoke test passed"
