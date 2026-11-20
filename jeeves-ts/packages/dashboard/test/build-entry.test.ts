import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

test("the dashboard build requires a stamped entry even when the stylesheet is rewritten", (t) => {
  const root = mkdtempSync(join(tmpdir(), "jeeves-build-fixture-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, "src/ui/assets"), { recursive: true });
  mkdirSync(join(root, "dist"));
  for (const name of ["copy-assets.mjs", "src/sw.js", "src/manifest.webmanifest", "src/ui/assets/jeeves-icon.svg"]) {
    copyFileSync(new URL("../" + name, import.meta.url), join(root, name));
  }
  writeFileSync(join(root, "dist/build.css"), "");
  const build = (entry: string) => {
    writeFileSync(join(root, "dist/index.html"), `<link href="build.css"><script src="${entry}"></script>`);
    return spawnSync(process.execPath, ["copy-assets.mjs"], { cwd: root, encoding: "utf8" });
  };
  assert.notEqual(build("missing.js").status, 0);
  assert.equal(build("./main.js").status, 0);
  const stamped = readFileSync(join(root, "dist/index.html"), "utf8").match(/main\.js\?v=\w+/);
  assert.ok(stamped);
  assert.ok(readFileSync(join(root, "dist/sw.js"), "utf8").includes(stamped[0]),
    "the first install must precache the same entry as the offline shell");
});
