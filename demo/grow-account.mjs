// GROW the single controller-account cell in place (capacity top-up WITHOUT
// creating a second account cell — the mistake topup.mjs made; the lock allows
// only one ACCOUNT input per tx, but other-lock inputs alongside it are fine,
// as the committed Fiber funding tx proved).
//
// tx: [account cell (session witness), one plain sighash cell (CCC witness)]
//     -> [account cell grown by GROW_CKB, change back to the sighash key]
// Outflow from the account is 0 (it only gains), so the spend cap is untouched.
//
// Account lock, deps, keys and RPC come from controller.config.json / the manifest.
//
// usage: node grow-account.mjs [growCkb] [send]
import { ccc } from "@ckb-ccc/core";
import { secp256k1 } from "@noble/curves/secp256k1";
import { accountLock, makeSigner, LOCK, AUTH_DEP, SESSION_PRIV, explorer } from "./controller-config.mjs";

const GROW = ccc.fixedPointFrom(process.argv[2] && !isNaN(Number(process.argv[2])) ? process.argv[2] : "700");
const SEND = process.argv.includes("send");
const FEE = 100000n; // 0.001 CKB

const { client, signer } = await makeSigner();
const { lock: accLock, wasm } = await accountLock();
const { tx_message, session_witness_registered, channel_proof_region } = wasm;

// the single account cell
let account = null,
  count = 0;
for await (const cell of client.findCellsByLock(accLock, null, true)) {
  account = cell;
  count++;
}
if (!account) throw new Error("no account cell");
if (count !== 1) throw new Error(`account must be a single cell (found ${count}) — run drain-account.mjs first`);
const accCap = account.cellOutput.capacity;
console.log(`account: ${account.outPoint.txHash}:${BigInt(account.outPoint.index)} = ${accCap / 100000000n} CKB`);

// one plain sighash cell covering GROW + change occupied (61) + fee
const sighashLock = (await signer.getRecommendedAddressObj()).script;
const need = GROW + ccc.fixedPointFrom(62) + FEE;
let feeCell = null;
for await (const cell of client.findCellsByLock(sighashLock, null, true)) {
  if (cell.outputData === "0x" && !cell.cellOutput.type && cell.cellOutput.capacity >= need) {
    feeCell = cell;
    break;
  }
}
if (!feeCell) throw new Error(`no plain sighash cell with ${need / 100000000n} CKB`);
console.log(`sighash in: ${feeCell.outPoint.txHash}:${BigInt(feeCell.outPoint.index)} = ${feeCell.cellOutput.capacity / 100000000n} CKB`);

const tx = ccc.Transaction.from({
  inputs: [
    { previousOutput: account.outPoint, since: 0n },
    { previousOutput: feeCell.outPoint, since: 0n },
  ],
  cellDeps: [
    { outPoint: LOCK().dep, depType: "code" },
    { outPoint: AUTH_DEP(), depType: "code" },
  ],
  outputs: [
    { lock: accLock, capacity: accCap + GROW }, // the grown account (output 0)
    { lock: sighashLock, capacity: feeCell.cellOutput.capacity - GROW - FEE }, // change
  ],
  outputsData: ["0x", "0x"],
  witnesses: ["0x", "0x"],
});

// session witness for input 0: msg = blake2b(raw tx, cell_deps cleared) — witnesses
// are NOT covered, so filling witness 1 later doesn't invalidate it.
const msg = tx_message(ccc.hexFrom(tx.toBytes()));
const s = secp256k1.sign(msg.slice(2), SESSION_PRIV);
const sig = "0x" + Buffer.from([...s.toCompactRawBytes(), s.recovery]).toString("hex");
tx.witnesses[0] = session_witness_registered(sig, "", channel_proof_region());

console.log(`  -> account ${(accCap + GROW) / 100000000n} CKB, change ${(feeCell.cellOutput.capacity - GROW - FEE) / 100000000n} CKB`);

if (SEND) {
  // CCC signs only its sighash group (input 1) and adds the secp dep group.
  const hash = await signer.sendTransaction(tx);
  console.log("\nBROADCAST grow:", hash);
  console.log("explorer: " + explorer(hash));
} else {
  console.log("\nDRY RUN — add `send` to broadcast.");
}
