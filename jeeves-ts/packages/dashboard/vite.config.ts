import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
export default defineConfig({
  root: "src",
  base: "./",
  plugins: [vue()],
  resolve: { dedupe: ["vue", "lucide-vue-next"] },
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    rolldownOptions: {
      output: {
        entryFileNames: "main.js",
        chunkFileNames: "chunks/[name]-[hash].js",
      },
    },
  },
  server: { proxy: { "/api": "http://127.0.0.1:7777" } },
});
