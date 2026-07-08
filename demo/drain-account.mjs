// Drain the SMALL controller-account cell so the account is a single cell again.
// The lock allows only one account input per tx (Error::MultipleInputs), but a
// settle/topup can leave a 2nd cell — Fiber collected both and the funding tx was
// rejected (error 5). This session-signs a spend of the small cell back to the
// configured key's sighash lock; no continuation output is required
// (enforce_account_outputs allows returned=0) as long as the outflow fits the
// spend cap.
//
// Account lock, deps, keys and RPC come from controller.config.json / the manifest.
import { ccc } from "@ckb-ccc/core";
import { secp256k1 } from "@noble/curves/secp256k1";
import { accountLock, makeSigner, LOCK, AUTH_DEP, SESSION_PRIV, explorer } from "./controller-config.mjs";

const SEND = process.argv.includes("send");
const FEE = 100000n; // 0.001 CKB (a flat 1 CKB on a tiny tx exceeds the fee-rate cap)

const { client, signer } = await makeSigner();
const { lock: accLock, wasm } = await accountLock();
const { tx_message, session_witness_registered, channel_proof_region } = wasm;

// find the SMALLEST account cell (the one to drain)
let small = null;
for await (const cell of client.findCellsByLock(accLock, null, true)) {
  if (!small || cell.cellOutput.capacity < small.cellOutput.capacity) small = cell;
}
if (!small) throw new Error("no account cell found");
const cap = small.cellOutput.capacity;
console.log("draining cell:", small.outPoint.txHash + ":" + BigInt(small.outPoint.index), cap / 100000000n, "CKB");

// drained value returns to the configured key's own sighash lock
const destLock = (await signer.getRecommendedAddressObj()).script;
const tx = ccc.Transaction.from({
  inputs: [{ previousOutput: small.outPoint, since: 0n }],
  cellDeps: [
    { outPoint: LOCK().dep, depType: "code" },
    { outPoint: AUTH_DEP(), depType: "code" },
  ],
  outputs: [{ lock: destLock, capacity: cap - FEE }],
  outputsData: ["0x"],
  witnesses: ["0x"],
});

// controller message = blake2b(raw tx, cell_deps cleared) — via wasm over the molecule
const msg = tx_message(ccc.hexFrom(tx.toBytes()));
// session signs (recoverable secp256k1): r||s||recid
const s = secp256k1.sign(msg.slice(2), SESSION_PRIV);
const sig = "0x" + Buffer.from([...s.toCompactRawBytes(), s.recovery]).toString("hex");
tx.witnesses = [session_witness_registered(sig, "", channel_proof_region())];

console.log("  -> out", (cap - FEE) / 100000000n, "CKB to", destLock.codeHash.slice(0, 10), "(sighash)");
if (SEND) {
  const h = await client.sendTransaction(tx);
  console.log("\nBROADCAST:", h, "\nexplorer: " + explorer(h));
} else {
  console.log("\nDRY RUN — add `send` to broadcast.");
}
