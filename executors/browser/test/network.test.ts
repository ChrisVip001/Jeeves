import { test } from "node:test";
import assert from "node:assert/strict";
import { NetworkScope, publicAddress, proxy } from "../src/network.ts";
import http from "node:http";

test("network scope rejects private, metadata, mapped IPv6 and reserved destinations", async () => {
  for (const ip of [
    "127.0.0.1",
    "10.0.0.1",
    "169.254.169.254",
    "100.64.0.1",
    "192.168.1.1",
    "192.0.2.1",
    "192.88.99.1",
    "0.0.0.0",
    "198.18.0.1",
    "::1",
    "::ffff:8.8.8.8",
    "fc00::1",
    "2001:db8::1",
    "2002:a00:1::1",
    "3fff::1",
  ])
    assert.equal(publicAddress(ip), false, ip);
  for (const ip of ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"])
    assert.equal(publicAddress(ip), true, ip);
  const scope = new NetworkScope(["http://127.0.0.1:4444"], false);
  await assert.rejects(
    scope.destination(scope.permit("http://127.0.0.1:4444/page")),
    /Private/,
  );
  assert.throws(() => scope.permit("http://127.0.0.1:4445"), /outside/);
  assert.throws(() => scope.permit("http://secret@127.0.0.1:4444"), /outside/);
  assert.throws(
    () => new NetworkScope(["https://example.com/path"], false),
    /without paths/,
  );
  assert.throws(() => new NetworkScope(["file:///"], false), /Invalid/);
});

test("localhost access requires explicit scope, flag and exact origin; proxy pins destination", async () => {
  const target = http.createServer((_req, res) => res.end("local fixture"));
  await new Promise<void>((resolve) => target.listen(0, "127.0.0.1", resolve));
  const address = target.address();
  assert.ok(address && typeof address !== "string");
  const origin = `http://127.0.0.1:${address.port}`;
  const scope = new NetworkScope([origin], true);
  assert.equal(
    (await scope.destination(scope.permit(origin))).address,
    "127.0.0.1",
  );
  const transport = await proxy(scope);
  try {
    const request = (url: string) =>
      new Promise<{ status: number; body: string }>((resolve, reject) => {
        http
          .get(transport.url, { path: url }, (response) => {
            let body = "";
            response.on("data", (chunk) => (body += chunk));
            response.on("end", () =>
              resolve({ status: response.statusCode!, body }),
            );
          })
          .on("error", reject);
      });
    assert.deepEqual(await request(origin + "/page"), {
      status: 200,
      body: "local fixture",
    });
    assert.equal((await request("http://169.254.169.254/")).status, 403);
  } finally {
    await transport.close();
    await new Promise<void>((resolve) => target.close(() => resolve()));
  }
});
