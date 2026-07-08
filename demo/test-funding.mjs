// Node validation of the live-mode funding signer (no Fiber peer needed).
import { readFile } from "node:fs/promises";
import { ccc } from "@ckb-ccc/core";
import { secp256k1 } from "@noble/curves/secp256k1";
import init, {
  tx_message,
  session_witness_registered,
  channel_proof_region,
} from "./pkg/controller.js";

// init wasm from bytes (Node has no fetch for the .wasm URL)
const wasmBytes = await readFile(new URL("./pkg/controller_bg.wasm", import.meta.url));
await init({ module_or_path: wasmBytes });

// ---- inline copies of funding.ts logic (mjs can't import the .ts directly) ----
const toMolLike = (j) => ({
  version: j.version,
  cellDeps: j.cell_deps.map((d) => ({
    outPoint: { txHash: d.out_point.tx_hash, index: d.out_point.index },
    depType: d.dep_type === "dep_group" ? "depGroup" : d.dep_type,
  })),
  headerDeps: j.header_deps,
  inputs: j.inputs.map((i) => ({
    previousOutput: { txHash: i.previous_output.tx_hash, index: i.previous_output.index },
    since: i.since,
  })),
  outputs: j.outputs.map((o) => ({
    capacity: o.capacity,
    lock: { codeHash: o.lock.code_hash, hashType: o.lock.hash_type, args: o.lock.args },
    type: o.type ? { codeHash: o.type.code_hash, hashType: o.type.hash_type, args: o.type.args } : undefined,
  })),
  outputsData: j.outputs_data,
  witnesses: j.witnesses,
});
const molHex = (j) => ccc.hexFrom(ccc.Transaction.from(toMolLike(j)).toBytes());

// A representative external-funding tx: account input -> [fiber funding cell, change back to account].
const ACCOUNT_LOCK = { code_hash: "0x" + "9c".repeat(32), hash_type: "data2", args: "0x" + "de".repeat(53) };
const FIBER_FUNDING_LOCK = { code_hash: "0x" + "fb".repeat(32), hash_type: "type", args: "0x" + "01".repeat(32) };
const unsignedTx = {
  version: "0x0",
  cell_deps: [
    { dep_type: "code", out_point: { tx_hash: "0x" + "2d".repeat(32), index: "0x0" } }, // lock dep
    { dep_type: "code", out_point: { tx_hash: "0x" + "53".repeat(32), index: "0x0" } }, // auth dep
  ],
  header_deps: [],
  inputs: [{ previous_output: { tx_hash: "0x" + "11".repeat(32), index: "0x0" }, since: "0x0" }],
  outputs: [
    { capacity: "0x" + (300n * 100000000n).toString(16), lock: FIBER_FUNDING_LOCK }, // channel funding
    { capacity: "0x" + (699n * 100000000n).toString(16), lock: ACCOUNT_LOCK },        // change back
  ],
  outputs_data: ["0x", "0x"],
  witnesses: ["0x", "0x"],
};

let pass = 0, fail = 0;
const check = (name, cond) => { (cond ? (pass++, console.log("  PASS", name)) : (fail++, console.log("  FAIL", name))); };

// 1. wasm tx_message succeeds and is deterministic
const msg = tx_message(molHex(unsignedTx));
check("tx_message is 0x + 32 bytes", /^0x[0-9a-f]{64}$/.test(msg));
check("tx_message deterministic", msg === tx_message(molHex(unsignedTx)));

// 2. INDEPENDENT cross-check: the controller message == CKB tx-hash of the same tx
//    with cell_deps cleared (sdk::tx_message = blake2b256(RawTransaction, cell_deps emptied);
//    CCC .hash() is blake2b256(RawTransaction)). If these agree, our molecule matches the lock.
const clearedHash = ccc.Transaction.from(toMolLike({ ...unsignedTx, cell_deps: [] })).hash();
check("message == CCC hash of cell_deps-cleared tx", msg === clearedHash);
console.log("    msg        :", msg);
console.log("    clearedHash:", clearedHash);

// 3. sign + build witness, splice at the account input (index 0)
const sessionPriv = secp256k1.utils.randomPrivateKey();
const sig = (() => {
  const s = secp256k1.sign(msg.slice(2), sessionPriv);
  const out = new Uint8Array(65);
  out.set(s.toCompactRawBytes(), 0);
  out[64] = s.recovery;
  return "0x" + Buffer.from(out).toString("hex");
})();
const witness = session_witness_registered(sig, "", channel_proof_region());
const signed = { ...unsignedTx, witnesses: ["0x", "0x"] };
signed.witnesses[0] = witness;

check("witness is non-empty hex", /^0x[0-9a-f]+$/.test(witness) && witness.length > 4);
check("session signature (65 bytes) embedded in witness", witness.includes(sig.slice(2)));
check("inputs unchanged (witness-only)", JSON.stringify(signed.inputs) === JSON.stringify(unsignedTx.inputs));
check("outputs unchanged (witness-only)", JSON.stringify(signed.outputs) === JSON.stringify(unsignedTx.outputs));
check("message unchanged after adding witness", tx_message(molHex(signed)) === msg);

console.log(`\n${fail === 0 ? "ALL GREEN" : "FAILURES"}: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
