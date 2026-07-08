// Safe transfer from the sighash key to an address — pins a plain change cell so
// the lock/auth CODE cells (live deps under the same key) are never spent.
// usage: node fund.mjs <ckt-address> <amountCKB> [send]
import { ccc } from "@ckb-ccc/core";
import { makeSigner } from "./controller-config.mjs";

const DEST = process.argv[2];
const AMOUNT = Number(process.argv[3] || "500");
const SEND = process.argv.includes("send");
if (!DEST) throw new Error("usage: node fund.mjs <ckt-address> <amountCKB> [send]");

const { client, signer } = await makeSigner();
const lock = (await signer.getRecommendedAddressObj()).script;

// pick one plain change cell (dataLen=0) that covers amount + headroom
const need = ccc.fixedPointFrom(AMOUNT) + ccc.fixedPointFrom(100);
let chosen = null;
for await (const cell of client.findCellsByLock(lock, null, true)) {
  if (cell.outputData === "0x" && cell.cellOutput.capacity >= need) { chosen = cell; break; }
}
if (!chosen) throw new Error("no plain change cell with enough capacity");

const destLock = (await ccc.Address.fromString(DEST, client)).script;
const tx = ccc.Transaction.from({
  outputs: [{ lock: destLock, capacity: ccc.fixedPointFrom(AMOUNT) }],
  outputsData: ["0x"],
});
tx.inputs.push(ccc.CellInput.from({ previousOutput: chosen.outPoint, since: 0n }));
await tx.completeFeeBy(signer, 1000n);

// HARD SAFETY: exactly the pinned plain cell, nothing else.
const ok =
  tx.inputs.length === 1 &&
  tx.inputs[0].previousOutput.txHash === chosen.outPoint.txHash &&
  BigInt(tx.inputs[0].previousOutput.index) === BigInt(chosen.outPoint.index);
if (!ok) throw new Error("ABORT: unexpected inputs — refusing to risk a code cell");

console.log(`input ${chosen.outPoint.txHash}:${BigInt(chosen.outPoint.index)} (${chosen.cellOutput.capacity / 100000000n} CKB)`);
for (const o of tx.outputs) console.log("  out", ccc.fixedPointToString(o.capacity), "CKB ->", o.lock.codeHash.slice(0, 10));

if (SEND) {
  const h = await signer.sendTransaction(tx);
  console.log("\nBROADCAST:", h);
  console.log("explorer: https://testnet.explorer.nervos.org/transaction/" + h);
} else {
  console.log("\nDRY RUN — add `send` to broadcast.");
}
