#!/usr/bin/env python3
"""Every repo path a doc names in backticks must exist (docs rot by pointing at deleted code)."""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PATH_RE = re.compile(r"`((?:jeeves-rs|jeeves-ts|desktop|docs|scripts|evals)/[A-Za-z0-9_./-]+)`")
SKIP = ("planning", "node_modules")  # planning notes are dated records, not live docs


def main() -> int:
    bad = []
    files = [*ROOT.glob("*.md"), *ROOT.glob("docs/**/*.md")]
    for doc in sorted(files):
        if any(part in doc.parts for part in SKIP) or doc.name.startswith("CHANGELOG"):
            continue
        for n, line in enumerate(doc.read_text(encoding="utf-8").splitlines(), 1):
            for path in PATH_RE.findall(line):
                if not (ROOT / path.rstrip("/")).exists():
                    bad.append(f"{doc.relative_to(ROOT)}:{n}: {path}")
    if bad:
        print("docs name paths that do not exist:\n  " + "\n  ".join(bad))
        return 1
    print(f"✓ doc paths exist ({len(files)} files checked)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
