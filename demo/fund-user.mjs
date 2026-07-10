// Fund ONE per-user controller account cell (Phase 2: the address a browser
// shows in multi-user mode) from the sighash key — SAFELY. Two invariants:
//  - the sighash key also holds the lock/auth CODE cells (live cell-deps): pin
//    a single plain input (dataLen=0, no type) and hard-assert nothing else.
//  - the account lock rejects >1 account input (MultipleInputs), so a user
//    account must STAY a single live cell: refuse to fund an address that
//    already has one (growing it needs the user's session key — in the browser).
// usage: node fund-user.mjs <ckt-address> [amountCkb] [send]   (or USER_ADDR env)
import { ccc } from "@ckb-ccc/core";
import { makeSigner } from "./controller-config.mjs";

const args = process.argv.slice(2).filter((a) => a !== "send");
const SEND = process.argv.includes("send");
const DEST = args[0] ?? process.env.USER_ADDR;
// default 700: 500 channel budget + ~170 change-cell reserve + fee margin
const AMOUNT = Number(args[1] ?? "700");
if (!DEST) throw new Error("usage: node fund-user.mjs <ckt-address> [amountCkb] [send]  (or USER_ADDR=ckt…)");

const { client, signer } = await makeSigner();
const destLock = (await ccc.Address.fromString(DEST, client)).script;

// single-cell invariant: never create a second account cell
let existing = 0;
for await (const cell of client.findCellsByLock(destLock, null, true)) {
  console.log(`existing cell: ${cell.outPoint.txHash}:${BigInt(cell.outPoint.index)} (${cell.cellOutput.capacity / 100000000n} CKB)`);
  existing++;
}
if (existing > 0) {
  throw new Error(
    `address already has ${existing} live cell(s) — a second cell bricks channel opens (MultipleInputs). ` +
      `Grow the existing cell instead (needs the user's session key; see grow-account.mjs for the pattern).`,
  );
}

// pick one plain sighash cell (dataLen=0, no type) covering amount + headroom
const lock = (await signer.getRecommendedAddressObj()).script;
const need = ccc.fixedPointFrom(AMOUNT) + ccc.fixedPointFrom(100);
let chosen = null;
for await (const cell of client.findCellsByLock(lock, null, true)) {
  if (cell.outputData === "0x" && !cell.cellOutput.type && cell.cellOutput.capacity >= need) {
    chosen = cell;
    break;
  }
}
if (!chosen) throw new Error("no plain change cell with enough capacity");

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
