# Jeeves — task entrypoint for the layered repo (Rust runtime, Go services, web shells).
#
# Install just:  brew install just      (or cargo install just)
#
# Run the repository tasks from its root.

python := if os_family() == "windows" { "python" } else { "python3" }
root := justfile_directory()

# List all tasks
help:
    @just -l

# ---- Protocol: single source of truth ----------------------------------------------------
# Regenerate TypeScript and JSON Schema from the Rust types in protocol.
# Run this whenever the protocol changes, or the frontend silently drifts.
write-app-server-schema *args:
    {{ python }} {{ justfile_directory() }}/scripts/write_schema.py {{args}}

# Only check whether the generated artifacts are stale (CI; writes nothing)
check-app-server-schema:
    {{ python }} {{ justfile_directory() }}/scripts/write_schema.py --check

# ---- Docs: one number, three places ------------------------------------------------------
# The tool count appears in the code, in docs/internals/04-tools.md (inventory + heading) and
# in both READMEs. Three copies drift; this is the one that notices. Why a script and not a
# cargo test: these Markdown files are not inside a crate, and a packaged crate should not go
# red because a repository document was reworded.
check-env-reads:
    {{ python }} {{ justfile_directory() }}/scripts/check_env_reads.py

# The one-way boundaries in AGENTS.md are only worth as much as the check that
# fails when code drifts past them: no engine→shell dependency, no platform
# cfg outside the two OS-capability adapters, no shell importing another shell.
check-architecture:
    {{ python }} {{ justfile_directory() }}/scripts/check_architecture.py

# The codebase is the agent's long-term memory: a stale `//` becomes the next
# agent's example. Doc comments stay; corrections go into lint errors or test names.
check-comments:
    {{ python }} {{ justfile_directory() }}/scripts/check_comments.py

check-doc-paths:
    {{ python }} {{ justfile_directory() }}/scripts/check_doc_paths.py

check-doc-links:
    {{ python }} {{ justfile_directory() }}/scripts/check_doc_links.py

check-tool-docs:
    {{ python }} {{ justfile_directory() }}/scripts/check_tool_docs.py

# The dashboard's English table must cover every string the code uses. `t()` falls back to
# Chinese, so a missing entry never throws — it just leaves one Chinese label on an English
# screen, which is exactly the kind of thing nobody notices in review.
check-i18n:
    {{ python }} {{ justfile_directory() }}/scripts/check_i18n.py

# Every `JEEVES_*` the code can read must exist in `.env.example` and in both configuration docs.
# Same reason as above: four copies of one knob (code + sample + two docs) is four places to
# forget, and a knob missing from the sample is a feature nobody can turn on.
check-config-docs:
    {{ python }} {{ justfile_directory() }}/scripts/check_config_docs.py

# ---- Feishu CLI -------------------------------------------------------------
# Install the official Feishu CLI locally; native CLI credentials remain unchanged.
install-feishu-cli:
    npm install --prefix "{{ justfile_directory() }}/.jeeves/tools/lark-cli" --save-exact @larksuite/cli@1.0.100 --no-audit --no-fund

# Pass arguments to the installed CLI (for example: just feishu-cli auth status).
feishu-cli *args:
    "{{ justfile_directory() }}/.jeeves/tools/lark-cli/node_modules/.bin/lark-cli" {{args}}

# ---- Rust ------------------------------------------------------------------
fmt:
    cd {{ root }} && cargo fmt --all

fmt-check:
    cd {{ root }} && cargo fmt --all -- --check

# Reclaim build debris without losing compiled dependencies: incremental sessions
# (one directory per compilation unit per rebuild) and cross-target artefacts.
# Keeps `target/debug/deps`, so the next build only redoes what actually changed.
# `find -delete` rather than `rm -rf`: some environments wrap `rm` with a bulk-delete
# confirmation that a Makefile-style recipe cannot answer.
tidy:
    @find {{ root }}/target/debug/incremental -depth -delete 2>/dev/null || true
    @rm -rf {{ root }}/target/x86_64-* {{ root }}/target/aarch64-* 2>/dev/null || true
    @du -sh {{ root }}/target 2>/dev/null || true

clippy *args:
    cd {{ root }} && cargo clippy --workspace --all-targets {{args}}

fix *args:
    cd {{ root }} && cargo clippy --fix --allow-dirty --workspace --all-targets {{args}}

build *args:
    cd {{ root }} && cargo build --workspace {{args}}

# Regular tests. Protocol export tests are #[ignore]d (they write files); not run here.
test *args:
    cd {{ root }} && cargo test --workspace {{args}}

# Only the protocol crate
test-protocol *args:
    cd {{ root }} && cargo test -p jeeves-protocol {{args}}

# End to end: really start app-server, feed JSON-RPC via stdin, check every reply.
smoke: build dashboard-build
    {{ justfile_directory() }}/scripts/smoke.sh

# kill -9 mid-turn, then a fresh process reconciles and resumes the checkpoint (POSIX).
smoke-resume: build
    {{ justfile_directory() }}/scripts/smoke-resume.sh

# The live-reliability research case's publication gate, offline (no network, no model).
test-research-publication:
    {{ python }} {{ justfile_directory() }}/scripts/test-research-publication.py

# Real SIGKILL boundaries with isolated local provider/MCP fixtures (POSIX).
smoke-runtime-contract: build
    {{ python }} {{ justfile_directory() }}/scripts/smoke-runtime-contract.py

smoke-recovery: build
    {{ python }} {{ justfile_directory() }}/scripts/smoke-recovery.py

# Runtime process lifecycle and dynamic gateway rediscovery (POSIX).
smoke-lifecycle: build gateway-build
    {{ python }} {{ justfile_directory() }}/scripts/smoke-lifecycle.py

# ---- Evals: the diamond before a release --------------------------------------------------
# Deterministic eval: scripted models + assertions, offline 0/1. **Must pass 100%** —
# this is the release gate: exit 0 = releasable. Cases live in evals/deterministic/.
# Cases load from relative `evals/…` paths, so eval/judge run from the repo root.
eval: build
    cd {{ justfile_directory() }} && cargo run -q --manifest-path {{ root }}/Cargo.toml --bin jeeves -- eval

# Replay eval: recorded conversations + expected event order (compaction, approval timeout,
# streamed output, MCP disconnects). A gate like the deterministic eval: it pins "what happens
# before what", the thing most likely to drift unnoticed.
eval-replay: build
    cd {{ justfile_directory() }} && cargo run -q --manifest-path {{ root }}/Cargo.toml --bin jeeves -- eval --replay

# Long-task benchmarks: same scripted harness, asks "did autonomous task work get better or
# worse". **Records, never blocks** — exit 0 even when cases fail. Cases live in evals/long/.
bench: build
    cd {{ justfile_directory() }} && {{ root }}/target/debug/jeeves bench

# judge: a real model answers one turn; a small judge model scores 0-10 by rubric. Needs a key;
# the score doesn't block releases — it answers "how good", not "can we ship".
judge: build
    cd {{ justfile_directory() }} && cargo run -q --manifest-path {{ root }}/Cargo.toml --bin jeeves -- judge

# research-quality: real model researches unseen sources, a separate review round is decoded,
# and an independent judge scores the brief — each failure mode scored on its own. Needs a key;
# recorded only, never blocks a release. Cases live in evals/research/.
eval-research: build
    cd {{ justfile_directory() }} && cargo run -q --manifest-path {{ root }}/Cargo.toml --bin jeeves -- research

# ---- TypeScript ------------------------------------------------------------
# Install TS-side deps (npm workspace: client, gateways and dashboard frontend in one go)
ts-install:
    cd {{ root }} && npm install

# Type check and unit tests for the TS side. Generated artifacts are how TS consumes the protocol,
# so this also catches protocol drift — though the drift check above is the real guard.
ts-check: ts-install
    cd {{ root }} && npm run typecheck
    cd {{ root }} && npm test

# End to end: really start app-server, have the TS gateway ask it one thing.
smoke-gateway: build
    {{ justfile_directory() }}/scripts/smoke-gateway.sh

# Start the Lark gateway. Needs LARK_APP_ID and LARK_APP_SECRET; LARK_ALLOW takes open_ids
# (the ou_… string, not the App ID); add LARK_DOMAIN=lark for the international Lark.
# The app also needs bot capability enabled, a subscription to im.message.receive_v1, and
# **long connection as the subscription mode** — miss one and it "connects but never responds".
gateway-lark *args:
    cd {{ root }} && npm run gateway:lark {{args}}

# Telegram bot gateway (long polling; needs TELEGRAM_BOT_TOKEN, see docs/guide.md).
gateway-telegram *args:
    cd {{ root }} && npm run gateway:telegram {{args}}

# ---- Dashboard -----------------------------------------------------------------
# Build the Web workbench: Vue typecheck + Vite + static/PWA assets; output lands in
# web/dist, served by the Go gateway.
# Browsers can't read .ts, so this is the only TS package that needs building.
dashboard-build: ts-install
    cd {{ root }} && npm run build --workspace @jeeves/dashboard

# Offline authenticated Web workflow: typed settings, policies, stream and approval.
smoke-web: gateway-build
    cd {{ root }} && cargo build --bin jeeves
    {{ python }} {{ justfile_directory() }}/scripts/smoke-web.py

go-check:
    test -z "$(gofmt -l internal/cloudkit control-plane/*.go gateway)"
    go vet ./...
    go test -race ./...

gateway-build: dashboard-build
    go build -o {{ root }}/target/jeeves-gateway ./gateway

local-up: gateway-build
    cd {{ root }} && cargo build --bin jeeves
    {{ python }} {{ justfile_directory() }}/scripts/local-assistant.py

smoke-account: dashboard-build
    cd {{ root }} && cargo build --bin jeeves
    node {{ justfile_directory() }}/scripts/smoke-account.mjs

# Apple PWA approval fixtures in the bundled Chromium, including an offline cold launch.
smoke-approval-notice: gateway-build
    cd {{ root }} && cargo build --bin jeeves
    node {{ justfile_directory() }}/scripts/smoke-approval-notice.mjs

# Offline main conversation → task workspaces → follow-up and worker approval.
smoke-interaction *args: gateway-build
    cd {{ root }} && cargo build --bin jeeves
    {{ python }} {{ justfile_directory() }}/scripts/smoke-interaction.py {{args}}

# Legacy monitor cutover and a reusable numeric predicate on a scheduled source record.
smoke-price-watch *args:
    cd {{ root }} && cargo build -p jeeves-cli
    {{ python }} {{ justfile_directory() }}/scripts/smoke-price-watch.py {{args}}
    cd {{ root }} && cargo test -p jeeves-app-server scheduled_records_check_the_latest_signed_observation

# One npm workspace owns both the Web cockpit and the desktop UI.
desktop-install:
    cd {{ root }} && npm ci

# One vector source for the UI and all currently shipped platform icon formats.
desktop-icons:
    #!/usr/bin/env bash
    set -euo pipefail
    icon_tmp=$(mktemp -d)
    trap 'rm -rf "$icon_tmp"' EXIT
    cargo tauri icon web/src/ui/assets/jeeves-icon.svg --output "$icon_tmp"
    for asset in "$icon_tmp"/*; do
        if [[ -f "$asset" ]]; then cp "$asset" desktop/src-tauri/icons/; fi
    done

desktop-check:
    cd desktop/src-tauri && cargo fmt -- --check && cargo test && cargo clippy --all-targets -- -D warnings

# Offline provider fixture; no credentials or real user data are used.
smoke-desktop:
    cd {{ root }} && cargo build --bin jeeves
    {{ python }} {{ justfile_directory() }}/scripts/smoke-desktop.py

# Opt-in acceptance using the packaged app's foreground window and synthetic loopback services.
smoke-mac-cockpit: gateway-build desktop-build
    {{ python }} {{ justfile_directory() }}/scripts/smoke-mac-cockpit.py

# Open the checkout's Mac app or bring its running window to the front.
[macos]
desktop-open:
    #!/usr/bin/env bash
    set -euo pipefail
    app="{{justfile_directory()}}/desktop/src-tauri/target/release/bundle/macos/Jeeves.app"
    if [[ ! -d "$app" ]]; then
        just desktop-build
    fi
    /usr/bin/open "$app"

# Locate the checkout's running app and its Runtime without opening another window.
[macos]
desktop-status:
    #!/usr/bin/env bash
    set -euo pipefail
    bundle="{{justfile_directory()}}/desktop/src-tauri/target/release/bundle/macos/Jeeves.app/Contents/MacOS"
    processes="$(ps -axo pid=,ppid=,comm=)"
    app="$(awk -v binary="$bundle/jeeves-desktop" '
        {
            name=$0
            sub(/^[[:space:]]*[0-9]+[[:space:]]+[0-9]+[[:space:]]+/, "", name)
            if (name==binary) print $1
        }
    ' <<<"$processes")"
    if [[ -z "$app" || "$app" == *$'\n'* ]]; then
        echo 'Expected one running Jeeves.app from this checkout. Use just desktop-open to bring it up.' >&2
        exit 1
    fi
    printf 'App PID %s\nCheckout %s %s\n' "$app" "$(git branch --show-current)" "$(git rev-parse --short HEAD)"
    runtime="$(awk -v pid="$app" '$2==pid && $0 ~ /\/jeeves$/ {print $1}' <<<"$processes")"
    if [[ -z "$runtime" || "$runtime" == *$'\n'* ]]; then
        echo 'No single bundled Runtime child. Check the address in the existing window if it attaches to a running service.'
        exit 0
    fi
    printf 'Runtime PID %s\n' "$runtime"
    lsof -nP -a -p "$runtime" -iTCP -sTCP:LISTEN || true
    runtime_stderr="$(lsof -a -p "$runtime" -d 2 -Fn | awk '/^n/ {print substr($0,2)}' || true)"
    if [[ -n "$runtime_stderr" ]]; then
        printf 'Runtime stderr %s\n' "$runtime_stderr"
    fi
    engine="$(awk -v pid="$runtime" '$2==pid && $0 ~ /\/jeeves$/ {print $1}' <<<"$processes")"
    if [[ -n "$engine" && "$engine" != *$'\n'* ]]; then
        printf 'Engine PID %s\n' "$engine"
        files="$(lsof -a -p "$engine" -Fn || true)"
        database="$(awk '/^n.*\/state\.db$/ && !seen++ {print substr($0,2)}' <<<"$files")"
        if [[ -n "$database" ]]; then
            printf 'State %s\nTurn traces %s/traces\n' "$database" "${database%/*}"
        fi
    fi

# The development shell opens the Go gateway.
desktop-dev:
    cd {{ root }} && cargo build --bin jeeves
    cd desktop/src-tauri && cargo tauri dev

# Build the optional single-window adapter without starting AppKit or requesting permissions.
desktop-window-helper:
    mkdir -p target/debug
    xcrun swiftc -swift-version 5 -target "$(uname -m)-apple-macos14.0" -parse-as-library desktop/src-tauri/window/*.swift -o target/debug/jeeves-window
    @echo "{{justfile_directory()}}/target/debug/jeeves-window"

# Open two disposable native windows for operator acceptance on a Mac.
desktop-window-fixture: desktop-window-helper build gateway-build
    mkdir -p target/debug
    xcrun swiftc desktop/tests/WindowFixture.swift -o target/debug/JeevesWindowFixture
    {{ python }} scripts/manual-window.py

# Opt-in regression with real AppKit windows. Requires Accessibility and Screen Recording.
smoke-mac-window *args:
    #!/usr/bin/env bash
    set -euo pipefail
    out="{{justfile_directory()}}/target/native-window-check"
    mkdir -p "$out"
    xcrun swiftc desktop/tests/WindowFixture.swift -o "$out/JeevesWindowFixture"
    xcrun swiftc -swift-version 5 -target "$(uname -m)-apple-macos14.0" -parse-as-library desktop/src-tauri/window/{Window,CuaDriver}.swift desktop/tests/NativeWindowCheck.swift -o "$out/native-window-check"
    "$out/native-window-check" "$out/JeevesWindowFixture" {{args}}

# Bundle the local runtime with the desktop app, for the current Rust host target.
[positional-arguments]
desktop-build *configs:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release -p jeeves-cli --bin jeeves
    target=$(rustc -vV | sed -n 's/^host: //p')
    test -n "$target"
    mkdir -p desktop/src-tauri/binaries
    cp target/release/jeeves "desktop/src-tauri/binaries/jeeves-$target"
    flags=(--config sidecar.conf.json)
    if [[ "$(uname -s)" == Darwin ]]; then
        just desktop-window-helper
        cp target/debug/jeeves-window "desktop/src-tauri/binaries/jeeves-window-$target"
        flags+=(--config mac-window.conf.json)
    fi
    cd desktop/src-tauri
    cargo tauri build --bundles app "${flags[@]}" "$@"

[macos]
desktop-device-build origin: desktop-window-helper
    #!/usr/bin/env bash
    set -euo pipefail
    target=$(rustc -vV | sed -n 's/^host: //p')
    mkdir -p desktop/src-tauri/binaries
    CGO_ENABLED=0 go build -o "desktop/src-tauri/binaries/jeeves-device-$target" ./gateway
    cp target/debug/jeeves-window "desktop/src-tauri/binaries/jeeves-window-$target"
    cd desktop/src-tauri
    JEEVES_PUBLIC_ORIGIN="{{origin}}" cargo tauri build --bundles app --features cloud-device --config cloud-device.conf.json

# End to end: the Feishu connector against a scripted Feishu server, through the shipped binary.
smoke-connector: build
    {{ justfile_directory() }}/scripts/smoke-connector.sh

# End to end: the authenticated private Runtime API and SSE.
smoke-dashboard: build
    {{ justfile_directory() }}/scripts/smoke-dashboard.sh

# End to end: the business form — task API (/api/run), inbound webhooks, tier routing.
smoke-business: build
    {{ justfile_directory() }}/scripts/smoke-business.sh

# Package and execute the native CLI using the same path as release CI.
release-package *args:
    {{ python }} {{ justfile_directory() }}/scripts/package-release.py build {{ args }}

# An upgraded morning schedule retains its time zone and artifacts until the owner reviews it.
eval-morning-briefing:
    cd {{ root }} && cargo build -p jeeves-cli
    {{ python }} {{ justfile_directory() }}/scripts/smoke-price-watch.py --case morning

# Advance a controlled clock over persisted responsibility contracts and reopen the same database.
eval-responsibility:
    cd {{ root }} && cargo test -p jeeves-app-server responsibility::

# ---- Run all checks in one command ----------------------------------------------
# gate sits last: beyond green tests and lint, the deterministic eval must also pass 100%.
check: fmt-check go-check check-app-server-schema check-architecture check-comments check-doc-paths check-doc-links check-env-reads check-tool-docs check-config-docs check-i18n ts-check eval eval-replay eval-morning-briefing eval-responsibility
    cd {{ root }} && cargo clippy --workspace --all-targets -- -D warnings
    cd {{ root }} && cargo test --workspace

# Optional components: official Node + locked Playwright + its matching Chromium.
browser-runtime: ts-install
    {{ python }} {{ justfile_directory() }}/scripts/prepare-browser-runtime.py

# Browser-enabled app; the standard desktop-build remains lightweight.
desktop-build-browser: browser-runtime
    just desktop-build --config browser.conf.json

smoke-browser:
    cd {{ root }} && cargo build --bin jeeves
    PLAYWRIGHT_BROWSERS_PATH="{{ justfile_directory() }}/desktop/src-tauri/browser-runtime/browsers" JEEVES_BROWSER_REQUIRED=1 node --test executors/browser/test/*.test.ts
    {{ python }} {{ justfile_directory() }}/scripts/smoke-browser.py
    {{ python }} {{ justfile_directory() }}/scripts/smoke-remote-browser.py

# Resident research report with a loopback Feishu service and real approval/read-back.
smoke-weekly-report: build
    {{ python }} {{ justfile_directory() }}/scripts/smoke-weekly-report.py

# Idle resident MCP OAuth renewal against a loopback token issuer.
smoke-mcp-renewal: build
    {{ python }} {{ justfile_directory() }}/scripts/smoke-mcp-renewal.py

smoke-recurring-work *args: gateway-build
    cd {{ justfile_directory() }} && cargo build --bin jeeves
    {{ python }} {{ justfile_directory() }}/scripts/smoke-recurring-work.py {{args}}
