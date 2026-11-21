import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
export default defineConfig(({ mode }) => ({
  root: "src",
  base: "./",
  plugins: [vue()],
  resolve: { dedupe: ["vue", "lucide-vue-next"] },
  build: {
    outDir: mode === "account" ? "../dist/account" : "../dist",
    emptyOutDir: true,
    rolldownOptions: {
      input:
        mode === "account"
          ? new URL("./src/account.html", import.meta.url).pathname
          : undefined,
      output: {
        entryFileNames: mode === "account" ? "account.js" : "main.js",
        chunkFileNames: "chunks/[name]-[hash].js",
      },
    },
  },
}));
