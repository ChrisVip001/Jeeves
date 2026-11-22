import assert from "node:assert/strict";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { createApp, h } from "vue";
import { renderToString } from "vue/server-renderer";
import { createServer } from "vite";
import vue from "@vitejs/plugin-vue";

const server = await createServer({
  root: fileURLToPath(new URL("..", import.meta.url)),
  configFile: false,
  plugins: [vue()],
  server: { middlewareMode: true, watch: null, ws: false },
  optimizeDeps: { noDiscovery: true },
});
after(() => server.close());

for (const name of [
  "WorkspacePicker",
  "AttachmentPicker",
]) {
  test(`${name} never submits the surrounding chat composer`, async () => {
    const { default: component } = await server.ssrLoadModule(
      `/src/ui/${name}.vue`,
    );
    const html = await renderToString(
      createApp({
        render: () => h("form", [h(component, { busy: false })]),
      }),
    );
    const buttons = html.match(/<button\b[^>]*>/g) ?? [];
    assert.ok(buttons.length, "the composer control has a trigger");
    for (const button of buttons) {
      assert.match(
        button,
        /\btype="button"/,
        "opening a menu must preserve the message draft",
      );
    }
  });
}
