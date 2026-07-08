// Vendor the built wasm-bindgen package (../../wasm/pkg) into ./pkg so Vite can
// bundle it from inside the demo's project root. Run automatically before dev/build.
import { cpSync, mkdirSync, existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = resolve(here, "../../wasm/pkg");
const dst = resolve(here, "../pkg");

if (!existsSync(src)) {
  console.error(
    `wasm pkg not found at ${src}\n` +
      `build it first:  (from repo root)  ./scripts/build-wasm.sh`,
  );
  process.exit(1);
}

mkdirSync(dst, { recursive: true });
cpSync(src, dst, { recursive: true });
console.log(`copied ${src} -> ${dst}`);
