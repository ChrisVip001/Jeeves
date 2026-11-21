#!/usr/bin/env python3
"""Docs hygiene: relative links must resolve, and every English doc has a .zh.md twin
with the same heading structure (same number of headings per level)."""
import re, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SKIP = ("node_modules", "/target/", "/output/", ".git/")
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
HEAD = re.compile(r"^(#{1,6}) ", re.M)


def docs():
    """Working-tree formal docs only: root-level files and docs/ (skills, memory, fixtures are not docs)."""
    out = subprocess.run(["git", "ls-files", "-co", "--exclude-standard", "-z", "*.md"], cwd=ROOT, capture_output=True, text=True, check=True).stdout
    for name in out.split("\0"):
        if not name:
            continue
        if name.startswith("docs/") or "/" not in name:
            if not any(k in name for k in SKIP) and (ROOT / name).exists() and not (ROOT / name).is_symlink():
                yield ROOT / name


def strip_code(text):
    return re.sub(r"```.*?```", "", text, flags=re.S)


def shape(text):
    return [len(m.group(1)) for m in HEAD.finditer(strip_code(text))]


bad = []
for p in docs():
    text = strip_code(p.read_text())
    for target in LINK.findall(text):
        if re.match(r"[a-z]+:|#|mailto:", target):
            continue
        path = target.split("#")[0]
        if path and not (p.parent / path).exists():
            bad.append(f"{p.relative_to(ROOT)}: dead link {target}")
    if p.name.endswith(".zh.md"):
        continue
    twin = p.with_name(p.stem + ".zh.md")
    if "docs/design/" in str(p) or p.name in ("LICENSE.md",):
        continue
    if not twin.exists():
        bad.append(f"{p.relative_to(ROOT)}: no .zh.md twin")
    elif shape(p.read_text()) != shape(twin.read_text()):
        bad.append(f"{p.relative_to(ROOT)}: heading structure differs from its twin")
if bad:
    print("\n".join(bad))
    sys.exit(1)
print("✓ doc links resolve; every English doc has a twin with matching headings")
