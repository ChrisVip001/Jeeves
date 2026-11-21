#!/usr/bin/env python3
"""Build and verify Jeeves CLI release archives from the checkout."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile


ROOT = Path(__file__).resolve().parents[1]
RUST = ROOT / "jeeves-rs"
TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
)


def project():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"],
        cwd=RUST,
    ))
    version = next(package["version"] for package in metadata["packages"] if package["name"] == "jeeves-cli")
    return version, Path(metadata["target_directory"])


def archive_name(version, target):
    return f"jeeves-v{version}-{target}.tar.gz"


def package(version, build_dir, output, target):
    info = subprocess.check_output(["rustc", "-vV"], cwd=RUST, text=True)
    host = next(line.removeprefix("host: ") for line in info.splitlines() if line.startswith("host: "))
    target = target or host
    if target not in TARGETS:
        raise SystemExit(f"Unsupported Jeeves release target: {target}")
    build = ["cargo", "build", "--locked", "--release", "-p", "jeeves-cli"]
    if target != host:
        build.extend(["--target", target])
        build_dir /= target
    subprocess.run(build, cwd=RUST, check=True)

    output.mkdir(parents=True, exist_ok=True)
    final = output / archive_name(version, target)
    notes = ROOT / ".github" / "release-notes" / f"v{version}.md"
    with tempfile.TemporaryDirectory(prefix=".jeeves-package-", dir=output) as temporary:
        stage = Path(temporary)
        archive_path = stage / final.name
        with tarfile.open(archive_path, "w:gz") as archive:
            archive.add(build_dir / "release" / "jeeves", arcname="jeeves")
            archive.add(ROOT / "LICENSE", arcname="LICENSE")
            archive.add(notes, arcname="RELEASE.md")
        subprocess.run(["tar", "-xzf", str(archive_path), "-C", str(stage)], check=True)
        executable = stage / "jeeves"
        actual = subprocess.check_output([executable, "--version"], cwd=stage, text=True).strip()
        if actual != f"jeeves {version}":
            raise SystemExit(f"Packaged executable reported {actual!r}, expected Jeeves {version}")
        help_text = subprocess.check_output([executable, "--help"], cwd=stage, text=True)
        if "jeeves runtime" not in help_text:
            raise SystemExit("Packaged executable lacks the Runtime command")
        archive_path.replace(final)
    print(final)


def write_checksums(version, output):
    expected = {archive_name(version, target) for target in TARGETS}
    actual = {archive.name for archive in output.glob("*.tar.gz")}
    if actual != expected:
        raise SystemExit(f"Incomplete release assets. Missing: {sorted(expected - actual)}. Unexpected: {sorted(actual - expected)}.")
    entries = []
    for name in sorted(expected):
        sha256 = hashlib.sha256()
        with (output / name).open("rb") as archive:
            for block in iter(lambda: archive.read(1024 * 1024), b""):
                sha256.update(block)
        entries.append(f"{sha256.hexdigest()}  {name}\n")
    destination = output / "SHA256SUMS"
    destination.write_text("".join(entries), encoding="utf-8")
    print(destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("build", "checksums"))
    parser.add_argument("--target", choices=TARGETS)
    parser.add_argument("--tag")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    version, build_dir = project()
    if args.tag is not None and args.tag != f"v{version}":
        raise SystemExit(f"Release tag {args.tag!r} does not match workspace version v{version}")
    output = (args.output or build_dir / "release-artifacts").resolve()
    if args.action == "build":
        package(version, build_dir, output, args.target)
    else:
        write_checksums(version, output)


if __name__ == "__main__":
    main()
