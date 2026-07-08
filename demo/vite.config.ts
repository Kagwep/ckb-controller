import { defineConfig } from "vite";
import { resolve } from "node:path";
import { nodePolyfills } from "vite-plugin-node-polyfills";

// COOP/COEP make the page `crossOriginIsolated`, which the in-browser Fiber WASM
// node (@nervosnetwork/fiber-js) needs for SharedArrayBuffer. Harmless for the
// default mock mode.
const crossOriginIsolation = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

export default defineConfig({
  base: "./",
  // fiber-js (and CCC) are Node-oriented and reach for Buffer/global/process. Vite
  // externalizes Node builtins for the browser by default, so accessing them throws
  // ("Module 'buffer' has been externalized…"). Polyfill the globals they need.
  plugins: [
    nodePolyfills({
      globals: { Buffer: true, global: true, process: true },
      include: ["buffer", "process", "util", "stream", "events"],
    }),
  ],
  // fiber-js ships a ~14 MB bundle with inlined wasm + Web Workers; let Vite serve it
  // as-is instead of pre-bundling (which mangles the worker/wasm loading).
  optimizeDeps: { exclude: ["@nervosnetwork/fiber-js"] },
  // The file-linked @ckb-controller/sdk has its own node_modules — force one copy
  // of the shared deps (the polyfill shims only resolve from the demo's tree).
  resolve: { dedupe: ["@ckb-ccc/core", "@noble/curves", "@noble/hashes"] },
  // Two pages: index.html (buy-troops channel demo) + game.html (multiplayer board).
  build: {
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        game: resolve(__dirname, "game.html"),
      },
    },
  },
  server: {
    headers: crossOriginIsolation,
    // controller.config.json + .controller/manifest.json live at the repo root
    // (one dir above the vite root) and are statically imported by src/config.ts.
    fs: { allow: [resolve(__dirname, "..")] },
  },
  preview: { headers: crossOriginIsolation },
});
