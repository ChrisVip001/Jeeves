#!/usr/bin/env python3
"""Guard the one-way boundaries declared non-negotiable in AGENTS.md.

A violation reports the correct destination, not just "forbidden" — the message
is meant to steer the next edit the way a lint error steers an agent mid-task.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Credential storage and launching the OAuth browser are OS-capability adapters,
# not agent behaviour or state. Everywhere else the engine stays platform-free.
CFG_ALLOWLIST = {
    "runtime/crates/jeeves-config/src/models.rs",
    "runtime/crates/jeeves-connector/src/feishu_oauth.rs",
}
# Composition roots sit above the engine and may reach app-server.
SHELLS = {"jeeves-cli", "jeeves-eval"}

errors: list[str] = []


def rel(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def check_crate_edges() -> None:
    manifests = [*ROOT.glob("runtime/crates/*/Cargo.toml"), ROOT / "protocol/Cargo.toml", ROOT / "protocol/noop-macros/Cargo.toml"]
    for manifest in sorted(manifests):
        crate = manifest.parent.name
        deps = set()
        for line in manifest.read_text(encoding="utf-8").splitlines():
            m = re.match(r"^(jeeves-[a-z-]+)\s*=", line)
            if m:
                deps.add(m.group(1))
        if "jeeves-app-server" in deps and crate not in SHELLS:
            errors.append(
                f"{rel(manifest)}: `{crate}` depends on `jeeves-app-server`. "
                "The engine owns logic; shells compose it. Put shared behavior in "
                "`jeeves-protocol` or inject a trait, and let `jeeves-cli` wire it up."
            )


def check_platform_cfg() -> None:
    sources = [*ROOT.glob("runtime/crates/*/src/**/*.rs"), *ROOT.glob("protocol/src/**/*.rs"), *ROOT.glob("protocol/noop-macros/src/**/*.rs")]
    for src in sorted(sources):
        path = rel(src)
        if path in CFG_ALLOWLIST:
            continue
        for lineno, line in enumerate(src.read_text(encoding="utf-8").splitlines(), 1):
            if "#[cfg(" in line and "target_os" in line:
                errors.append(
                    f"{path}:{lineno}: platform `cfg` in the engine. "
                    "Platform differences live in shells/adapters; if this is an OS "
                    "capability (credentials, browser), extend `CFG_ALLOWLIST` with "
                    "a reason in AGENTS.md instead of spreading it."
                )


def check_ts_edges() -> None:
    packages: dict[str, set[str]] = {}
    workspace = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
    for package in workspace["workspaces"]:
        manifest = ROOT / package / "package.json"
        data = json.loads(manifest.read_text(encoding="utf-8"))
        name = data.get("name", rel(manifest))
        deps = set(data.get("dependencies", {})) | set(data.get("devDependencies", {}))
        packages[name] = {d for d in deps if d.startswith("@jeeves/")}

    if packages.get("@jeeves/client"):
        errors.append(
            "@jeeves/client must not depend on another @jeeves package: it is the "
            "shared bottom of the TS graph (transport + generated protocol types)."
        )
    shells = {"@jeeves/dashboard", "@jeeves/browser-worker"}
    for name, deps in packages.items():
        for dep in deps & shells:
            if dep != name:
                errors.append(
                    f"{name} depends on {dep}. Shells never import another shell — "
                    "share UI through the dashboard's `src/ui` and types through "
                    "`@jeeves/client`."
                )


def check_desktop_imports() -> None:
    for src in sorted((ROOT / "desktop").rglob("*.ts")):
        if "node_modules" in src.parts or "dist" in src.parts:
            continue
        for lineno, line in enumerate(src.read_text(encoding="utf-8").splitlines(), 1):
            if re.search(r"from\s+['\"]@jeeves/", line):
                errors.append(
                    f"{rel(src)}:{lineno}: desktop imports a TS package. The native "
                    "shell starts `jeeves runtime` and loads its address; it does "
                    "not link the browser front end."
                )


def main() -> None:
    check_crate_edges()
    check_platform_cfg()
    check_ts_edges()
    check_desktop_imports()
    if errors:
        print("architecture boundaries violated:")
        for error in errors:
            print(f"  - {error}")
        sys.exit(1)
    print("architecture boundaries OK")


if __name__ == "__main__":
    main()
