#!/usr/bin/env python3
"""Check documented source paths and the producers of optional build outputs."""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PATH_RE = re.compile(r"`((?:runtime|protocol|web|integrations|executors|gateway|control-plane|target|desktop|docs|scripts|evals|deploy)/[A-Za-z0-9_./-]+)`")
SKIP = ("planning", "node_modules")  # planning notes are dated records, not live docs
GENERATED_BY = {
    "target/debug/jeeves": "runtime/crates/jeeves-cli/Cargo.toml",
    "web/dist": "web/package.json",
    "desktop/src-tauri/browser-runtime": "scripts/prepare-browser-runtime.py",
    "desktop/src-tauri/target/release/bundle/macos/Jeeves.app": "desktop/src-tauri/tauri.conf.json",
}


def main() -> int:
    bad = []
    files = [*ROOT.glob("*.md"), *ROOT.glob("docs/**/*.md")]
    for doc in sorted(files):
        if any(part in doc.parts for part in SKIP) or doc.name.startswith("CHANGELOG"):
            continue
        for n, line in enumerate(doc.read_text(encoding="utf-8").splitlines(), 1):
            for path in PATH_RE.findall(line):
                target = path.rstrip("/")
                if not (ROOT / GENERATED_BY.get(target, target)).exists():
                    bad.append(f"{doc.relative_to(ROOT)}:{n}: {path}")
    if bad:
        print("docs name paths that do not exist:\n  " + "\n  ".join(bad))
        return 1
    print(f"✓ doc paths exist ({len(files)} files checked)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
