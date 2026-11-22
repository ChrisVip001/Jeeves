#!/usr/bin/env python3
"""Environment variables are read in jeeves-config, at boot. Anything else must be listed here.

A tenant is a JEEVES_HOME. A value read from the process environment belongs to the host, not to
a tenant, so every read outside jeeves-config is either host-level (the same for every tenant:
PATH, the port, bundled packs) or a credential looked up by name (which must go through the
secret store once it can be backed by something other than the host environment).
A new read fails this check until someone decides which of the two it is.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
READ = re.compile(r"env::var(?:_os|s)?\(\s*(\"[A-Z_]+\"|[a-z_.()]+)?")

# file -> what it reads and why that is the host's, not a tenant's.
HOST = {
    "runtime/crates/jeeves-app-server/src/server/mcp_live.rs": "MCP hot rebuild: JEEVES_MCP_CONFIG operator-level extra config paths, same as jeeves-mcp",
    "runtime/crates/jeeves-app-server/src/capability.rs": "capability probes: JEEVES_MCP_CONFIG bundled packs; credentials looked up by name",
    "runtime/crates/jeeves-app-server/src/route.rs": "JEEVES_MODEL_ROUTE: operator override for unattended runs",
    "runtime/crates/jeeves-app-server/src/server/search_mcp.rs": "JEEVES_SEARCH_MCP: operator switch",
    "runtime/crates/jeeves-app-server/src/browser/process.rs": "inherits the host environment for the browser child",
    "runtime/crates/jeeves-mcp/src/lib.rs": "JEEVES_MCP_CONFIG bundled packs; ${VAR} credentials by name",
    "runtime/crates/jeeves-memory/src/skills.rs": "JEEVES_SKILL_DIRS: bundled skill packs",
    "runtime/crates/jeeves-ops/src/client.rs": "JEEVES_WRITE / JEEVES_APPROVAL defaults for the dashboard child",
    "runtime/crates/jeeves-ops/src/lib.rs": "JEEVES_PORT and JEEVES_OPS_TOKEN: the private Runtime listener",
    "runtime/crates/jeeves-ops/src/gateways.rs": "JEEVES_GATEWAY_BIN: where the host finds the gateway script",
    "runtime/crates/jeeves-ops/src/capabilities.rs": "JEEVES_MCP_CONFIG bundled packs",
    "runtime/crates/jeeves-ops/src/webhooks.rs": "hook secrets by name (credential)",
    "runtime/crates/jeeves-ops/src/api/files.rs": "HOME for the file picker's starting point",
    "runtime/crates/jeeves-connector/src/config.rs": "connector secrets by name (credential)",
    "runtime/crates/jeeves-tools/src/exec.rs": "PATH to find the sandbox binary",
    "runtime/crates/jeeves-tools/src/web.rs": "search keys by name (credential) and operator switches",
    "runtime/crates/jeeves-tools/src/fs.rs": "HOME for ~ expansion",
    "protocol/src/export.rs": "schema generator (build tool)",
    "runtime/crates/jeeves-cli/src/main.rs": "CLI entry point",
    "runtime/crates/jeeves-cli/src/skill_cmd.rs": "CLI entry point",
    "runtime/crates/jeeves-cli/src/service_cmd.rs": "JEEVES_PORT for the unit and HOME for where the supervisor looks: the host's, not a tenant's",
    "runtime/crates/jeeves-eval/src/lib.rs": "eval harness",
}


def main() -> int:
    bad = []
    sources = [*ROOT.glob("runtime/crates/*/src/**/*.rs"), *ROOT.glob("protocol/src/**/*.rs")]
    for path in sorted(sources):
        rel = str(path.relative_to(ROOT))
        if rel.startswith("runtime/crates/jeeves-config/") or "test" in path.name:
            continue
        text = path.read_text(encoding="utf-8")
        text = text.split("#[cfg(test)]", 1)[0]
        for n, line in enumerate(text.splitlines(), 1):
            if line.lstrip().startswith("//") or not READ.search(line):
                continue
            if rel not in HOST:
                bad.append(f"{rel}:{n}: {line.strip()}")
    if bad:
        print(
            "environment read outside jeeves-config; read it into Settings at boot, "
            "or list the file in scripts/check_env_reads.py as host-level:\n  " + "\n  ".join(bad)
        )
        return 1
    print(f"✓ environment reads: only jeeves-config and {len(HOST)} host-level files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
