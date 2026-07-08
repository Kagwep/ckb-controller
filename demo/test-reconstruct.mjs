// Reconstruct the DEPLOYED controller account from the CLI's fixed keys and check
// it byte-for-byte against the on-chain cell.
import { readFile } from "node:fs/promises";
import { secp256k1 } from "@noble/curves/secp256k1";
import { blake2b } from "@noble/hashes/blake2b";
import { bytesToHex, utf8ToBytes } from "@noble/hashes/utils";
import init, {
  session_params,
  registered_args,
  wildcard_root,
  no_expiry,
  controller_address,
  script,
} from "./pkg/controller.js";

await init({ module_or_path: await readFile(new URL("./pkg/controller_bg.wasm", import.meta.url)) });

const CKB_HASH_PERSONAL = utf8ToBytes("ckb-default-hash");
const pubHash = (priv) =>
  "0x" + bytesToHex(blake2b(secp256k1.getPublicKey(priv, true), { dkLen: 32, personalization: CKB_HASH_PERSONAL }).slice(0, 20));

// Keys + lock come from controller.config.json / the manifest; ONCHAIN_ARGS below
// stays a hardcoded fixture (the byte-for-byte expectation this test checks).
import { OWNER_PRIV, SESSION_PRIV, LOCK, CONFIG } from "./controller-config.mjs";
const LOCK_CODE_HASH = LOCK().codeHash;
const HT_DATA2 = 0x04;
const CAP_1000_CKB = (BigInt(CONFIG.session.spendCapCkb) * 100000000n).toString(); // shannons
const ZERO_GUARDIAN = "0x" + "00".repeat(20);

const ownerHash = pubHash(OWNER_PRIV);
const sessionHash = pubHash(SESSION_PRIV);

const params = session_params(sessionHash, no_expiry(), wildcard_root(), CAP_1000_CKB, ZERO_GUARDIAN);
const args = registered_args(ownerHash, params);
const addr = controller_address(LOCK_CODE_HASH, HT_DATA2, args, true /* testnet */);
const lockMol = script(LOCK_CODE_HASH, HT_DATA2, args); // molecule-serialized Script hex

const ONCHAIN_ARGS = "0xf949a9cc83edefcd580eb3f0f3bae187c4d008dba1d89d8870b116ec25fb33a8c1f762ae6dcc5238ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff00e876481700000000000000000000000000000000000000000000000000000000000000";

let pass = 0, fail = 0;
const check = (n, c, got, exp) => { c ? (pass++, console.log("  PASS", n)) : (fail++, console.log("  FAIL", n, "\n    got:", got, "\n    exp:", exp)); };

console.log("owner   hash:", ownerHash);
console.log("session hash:", sessionHash);
check("owner_hash matches on-chain (args[0:20])", ownerHash === "0x" + ONCHAIN_ARGS.slice(2, 42), ownerHash, "0x" + ONCHAIN_ARGS.slice(2, 42));
check("session_hash matches on-chain (args[20:40])", sessionHash === "0x" + ONCHAIN_ARGS.slice(42, 82), sessionHash, "0x" + ONCHAIN_ARGS.slice(42, 82));
check("full registered args == on-chain cell args", args.toLowerCase() === ONCHAIN_ARGS.toLowerCase(), args, ONCHAIN_ARGS);
console.log("derived address:", addr);
console.log("lock molecule:", lockMol.slice(0, 24), "…");

console.log(`\n${fail === 0 ? "ALL GREEN" : "FAILURES"}: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
