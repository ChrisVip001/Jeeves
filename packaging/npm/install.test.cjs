const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

test("the release installer follows a relative redirect and reports HTTP failures", { skip: !["darwin", "linux"].includes(process.platform) }, (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "jeeves-installer-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  for (const file of ["install.js", "package.json"]) fs.copyFileSync(path.join(__dirname, file), path.join(dir, file));
  fs.writeFileSync(path.join(dir, "jeeves"), "fixture binary");
  execFileSync("tar", ["-czf", "release.tar.gz", "jeeves"], { cwd: dir });
  fs.writeFileSync(path.join(dir, "mock.cjs"), `
    const fs = require("node:fs");
    const { Readable } = require("node:stream");
    const { EventEmitter } = require("node:events");
    require("node:https").get = (input, options, receive) => {
      const url = new URL(input);
      const archive = url.pathname === "/fixture/archive" && url.hostname === "github.com";
      const response = archive ? fs.createReadStream("release.tar.gz") : Readable.from([]);
      response.statusCode = process.env.FIXTURE_HTTP_ERROR ? 404 : archive ? 200 : 302;
      response.headers = {location: "/fixture/archive"};
      queueMicrotask(() => receive(response));
      return new EventEmitter();
    };
  `);
  const run = (fail) => spawnSync(process.execPath, ["--require", "./mock.cjs", "./install.js"], {
    cwd: dir, encoding: "utf8", env: { PATH: process.env.PATH, FIXTURE_HTTP_ERROR: fail ? "1" : "" },
  });
  const missing = run(true);
  assert.equal(missing.status, 1);
  assert.match(missing.stderr, /HTTP 404/);
  assert.equal(fs.existsSync(path.join(dir, "bin", "jeeves")), false);
  const installed = run(false);
  assert.equal(installed.status, 0, installed.stderr);
  assert.equal(fs.readFileSync(path.join(dir, "bin", "jeeves"), "utf8"), "fixture binary");
  fs.accessSync(path.join(dir, "bin", "jeeves"), fs.constants.X_OK);
});
