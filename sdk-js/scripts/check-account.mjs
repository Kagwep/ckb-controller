// Phase-1 regression check (no test infra in this package): run after `npm run
// build`, plain node, no chain. Proves the per-user keys seam:
//   (a) the fixed config-key path is byte-identical to pre-change HEAD, and
//   (b) freshly generated key sets yield distinct accounts/addresses/args.
// Loads the built dist submodules directly (avoids the live.ts fiber-js chain)
// and the canonical wasm build at wasm/pkg. Wire formats: docs/internals/.
import { readFile } from "node:fs/promises";
import { deriveAccount } from "../dist/account.js";
import { genKey } from "../dist/keys.js";

const ROOT = new URL("../../", import.meta.url); // repo root

const CONFIG = JSON.parse(await readFile(new URL("controller.config.json", ROOT), "utf8"));
const MANIFEST = JSON.parse(await readFile(new URL(".controller/manifest.json", ROOT), "utf8"));
const NET = MANIFEST[CONFIG.network];

const wasm = await import("../../wasm/pkg/controller.js");
await wasm.default({ module_or_path: await readFile(new URL("wasm/pkg/controller_bg.wasm", ROOT)) });

// Regression constant: the fixed-key account address derived from UNMODIFIED HEAD
// (captured via `node demo/test-reconstruct.mjs` before the keys-override change).
// The absent-keys path MUST stay byte-identical to this — Node drivers/tests rely on it.
const FIXED_ADDR =
  "ckt1qzwneclzn3j5vl7l70kwywyru499lvp7va7em2qg09534xprqd9fcp8efx5ueqldalx4sr4n7rem4cv8cngq3kapmzwcsu93zmkzt7en4rqlwc4wdhx9yw8lllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllcqapmys9cqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqrhdq7q";

let pass = 0, fail = 0;
const check = (name, ok, got, exp) =>
  ok ? (pass++, console.log("  PASS", name)) : (fail++, console.log("  FAIL", name, "\n    got:", got, "\n    exp:", exp));

const fixed = deriveAccount(CONFIG, NET, wasm);
console.log("fixed-key address:", fixed.address);
check("fixed-key address unchanged (regression)", fixed.address === FIXED_ADDR, fixed.address, FIXED_ADDR);

const a = deriveAccount(CONFIG, NET, wasm, { owner: genKey(), session: genKey() });
const b = deriveAccount(CONFIG, NET, wasm, { owner: genKey(), session: genKey() });
console.log("generated A address:", a.address);
console.log("generated B address:", b.address);

check("A differs from fixed", a.address !== FIXED_ADDR, a.address, "!= " + FIXED_ADDR);
check("B differs from fixed", b.address !== FIXED_ADDR, b.address, "!= " + FIXED_ADDR);
check("A differs from B (address)", a.address !== b.address, a.address, "!= " + b.address);
check("A differs from B (args)", a.args !== b.args, a.args, "!= " + b.args);

console.log(`\n${fail === 0 ? "ALL GREEN" : "FAILURES"}: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
