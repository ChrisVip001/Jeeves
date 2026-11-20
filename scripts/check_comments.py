#!/usr/bin/env python3
"""Keep ordinary comments out of source: the codebase is the agent's memory.

Doc comments (`///`, `//!`, `/** */`) are structural and stay. Ordinary comments
rot into stale examples that agents copy; corrections belong in lint errors or
test names. An intentional exception must be listed in ALLOWLIST with a reason.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Format: "relative/path": "reason". Empty by design; add sparingly.
ALLOWLIST: dict[str, str] = {}

SKIP = ("/target/", "/node_modules/", "/dist/", "/.git/", "/schema/", "/coverage/")
IDENT = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_")


def is_ident(c: str) -> bool:
    return c in IDENT


def line_end(src: str, i: int) -> int:
    while i < len(src) and src[i] != "\n":
        i += 1
    return i


def quoted_end(src: str, i: int, quote: str) -> int:
    n = len(src)
    i += 1
    while i < n:
        if src[i] == "\\":
            i += 2
        elif src[i] == quote:
            return i + 1
        else:
            i += 1
    return n


def template_end(src: str, i: int) -> int:
    n = len(src)
    i += 1
    while i < n:
        if src[i] == "\\":
            i += 2
        elif src[i] == "`":
            return i + 1
        else:
            i += 1
    return n


def raw_string_end(src: str, i: int) -> int | None:
    n = len(src)
    if src[i] in "bc":
        if i + 1 >= n or src[i + 1] != "r":
            return None
        i += 2
    elif src[i] == "r":
        i += 1
    else:
        return None
    hashes = 0
    while i < n and src[i] == "#":
        hashes += 1
        i += 1
    if i >= n or src[i] != '"':
        return None
    close = '"' + "#" * hashes
    k = src.find(close, i + 1)
    return n if k < 0 else k + len(close)


def char_literal_end(src: str, i: int) -> int | None:
    n = len(src)
    i += 1
    if i >= n:
        return None
    if src[i] == "\\":
        i += 1
        if i >= n:
            return None
        c = src[i]
        if c == "u" and i + 1 < n and src[i + 1] == "{":
            k = src.find("}", i + 2)
            if k < 0:
                return None
            i = k + 1
        elif c == "x":
            i += 3
        else:
            i += 1
        return i + 1 if i < n and src[i] == "'" else None
    return i + 2 if i + 1 < n and src[i + 1] == "'" else None


def block_end_nested(src: str, i: int) -> int:
    n = len(src)
    depth = 0
    while i < n - 1:
        if src[i : i + 2] == "/*":
            depth += 1
            i += 2
        elif src[i : i + 2] == "*/":
            depth -= 1
            i += 2
            if depth == 0:
                return i
        else:
            i += 1
    return n


def ordinary_comments(src: str, rust: bool) -> list[int]:
    """Return 1-based line numbers of ordinary comments, skipping doc comments.

    Rust doc comments are `///`/`//!`. TS has no line doc comments — JSDoc is
    `/** */` — so `///` is ordinary there. TS template literals are not tracked:
    a backtick inside a regex or a comment previously put the scanner into a
    false string state and hid real comments from this check.
    """
    found: list[int] = []
    i = 0
    n = len(src)
    while i < n:
        c = src[i]
        if rust and (i == 0 or not is_ident(src[i - 1])) and c in "rbc":
            end = raw_string_end(src, i)
            if end is not None:
                i = end
                continue
        if c == '"':
            i = quoted_end(src, i, '"')
            continue
        if rust and c == "'":
            end = char_literal_end(src, i)
            if end is not None:
                i = end
                continue
        if not rust and c in ("'", '"'):
            i = quoted_end(src, i, c)
            continue
        if c == "/" and i + 1 < n:
            if src[i + 1] == "/":
                k = 2
                while i + k < n and src[i + k] == "/":
                    k += 1
                end = line_end(src, i)
                keep = (
                    (k == 3 or (k == 2 and i + 2 < n and src[i + 2] == "!"))
                    if rust
                    else False
                )
                if not keep:
                    found.append(src.count("\n", 0, i) + 1)
                i = end
                continue
            if src[i + 1] == "*":
                if src[i : i + 4] == "/**/":
                    found.append(src.count("\n", 0, i) + 1)
                    i += 4
                    continue
                end = block_end_nested(src, i) if rust else (src.find("*/", i + 2) + 2)
                third = src[i + 2] if i + 2 < n else ""
                if third not in ("*", "!"):
                    found.append(src.count("\n", 0, i) + 1)
                i = end
                continue
        i += 1
    return found


def collect_files() -> list[Path]:
    files: list[Path] = []
    for base in ("jeeves-rs", "desktop"):
        for dirpath, _, names in os.walk(ROOT / base):
            for name in names:
                if name.endswith(".rs"):
                    p = Path(dirpath) / name
                    rel = "/" + p.relative_to(ROOT).as_posix()
                    if not any(s in rel for s in SKIP):
                        files.append(p)
    for base in ("jeeves-ts", "control-plane", "desktop"):
        for dirpath, _, names in os.walk(ROOT / base):
            for name in names:
                if name.endswith((".ts", ".mts")):
                    p = Path(dirpath) / name
                    rel = "/" + p.relative_to(ROOT).as_posix()
                    if not any(s in rel for s in SKIP):
                        files.append(p)
    return sorted(files)


def main() -> None:
    violations: list[str] = []
    for path in collect_files():
        rel = path.relative_to(ROOT).as_posix()
        if rel in ALLOWLIST:
            continue
        src = path.read_text(encoding="utf-8")
        lines = ordinary_comments(src, path.suffix == ".rs")
        if not path.suffix == ".rs":
            # Without template parsing, `http://` inside a template is reported
            # as a comment. Only line-leading `//` is a comment in this repo.
            lines = [n for n in lines if src.splitlines()[n - 1].lstrip().startswith("//")]
        if lines:
            where = ", ".join(str(n) for n in lines[:8])
            more = f" (+{len(lines) - 8} more)" if len(lines) > 8 else ""
            violations.append(f"{rel}:{where}{more}")
    if violations:
        print("ordinary comments found (the codebase is the agent's memory):")
        for v in violations:
            print(f"  - {v}")
        print("Move the reason into a lint error, a test name, or a doc comment.")
        sys.exit(1)
    print(f"comment discipline OK ({len(collect_files())} source files checked)")


if __name__ == "__main__":
    main()
