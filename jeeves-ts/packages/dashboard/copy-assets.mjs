// Keep stable static/PWA URLs beside the Vite bundle and stamp its entry for cache safety.

import { copyFile, readFile, writeFile } from "node:fs/promises";

const here = import.meta.dirname; // script sits at the package root, beside src/

await copyFile(`${here}/src/manifest.webmanifest`, `${here}/dist/manifest.webmanifest`);

const icon = await readFile(`${here}/src/ui/assets/jeeves-icon.svg`, "utf8");
const tile = icon.match(/<rect\b[^>]*fill="([^"]+)"[^>]*\/>/);
if (!tile)
  throw new Error("Jeeves icon needs a background tile for the maskable variant");
await writeFile(`${here}/dist/icon.svg`, icon);
// Maskable icons need opaque edges; preserve the shared mark and background color.
await writeFile(
  `${here}/dist/icon-maskable.svg`,
  icon.replace(tile[0], `<rect width="1024" height="1024" fill="${tile[1]}"/>`),
);

const build = Date.now().toString(36);
const htmlPath = `${here}/dist/index.html`;
const html = await readFile(htmlPath, "utf8");
const css = html.match(/href="([^"\n]+\.css)"/);
if (!css) throw new Error("Vite did not emit the workbench stylesheet");
await copyFile(`${here}/dist/${css[1]}`, `${here}/dist/style.css`);
const main = `./main.js?v=${build}`;
const entry = html.replace(/src="(?:\.\/)?main\.js"/, `src="${main}"`);
if (entry === html) throw new Error("Vite did not emit the main.js entry in index.html");
const stamped = entry
  .replace(css[0], 'href="./style.css"')
  .replace(/href="[^"\n]+\.webmanifest"/, 'href="./manifest.webmanifest"');
await writeFile(htmlPath, stamped);

const worker = await readFile(`${here}/src/sw.js`, "utf8");
await writeFile(`${here}/dist/sw.js`, worker.replace('const ENTRY = "./main.js";', `const ENTRY = "${main}";`));
