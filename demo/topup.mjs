// Top up the deployed controller account cell from the sighash key — SAFELY.
// The sighash key also holds the lock/auth CODE cells (live cell-deps); pin the
// input to the plain change cell and assert no other input is pulled in.
import { ccc } from "@ckb-ccc/core";
import { readFile } from "node:fs/promises";

const SEND = process.argv.includes("send");
const TOPUP_CKB = 1300;

// the safe plain-change input identified by the cell listing
const CHANGE_CELL = {
  txHash: "0x1699b51f69dbcb46de27ea868dbb30b7f8b948f688eb5232c3a95842802b0571",
  index: "0x1",
};
// the deployed account lock (verified byte-for-byte on-chain)
const ACCOUNT_LOCK = {
  codeHash: "0x9d3ce3e29c65467fdff3ece23883e54a5fb03e677d9da80879691a9823034a9c",
  hashType: "data2",
  args:
    "0xf949a9cc83edefcd580eb3f0f3bae187c4d008dba1d89d8870b116ec25fb33a8c1f762ae6dcc5238ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff00e876481700000000000000000000000000000000000000000000000000000000000000",
};

const priv = (await readFile("D:/projects/ckb-controller-cli/testnet-key.txt", "utf8")).trim();
const client = new ccc.ClientPublicTestnet({ url: "https://testnet.ckb.dev/rpc" });
const signer = new ccc.SignerCkbPrivateKey(client, "0x" + priv.replace(/^0x/, ""));

const accountLock = ccc.Script.from(ACCOUNT_LOCK);

const tx = ccc.Transaction.from({
  outputs: [{ lock: accountLock, capacity: ccc.fixedPointFrom(TOPUP_CKB) }],
  outputsData: ["0x"],
});
// pin the single safe input
tx.inputs.push(ccc.CellInput.from({ previousOutput: CHANGE_CELL, since: 0n }));

// change back to the sighash key; must NOT add any other input
await tx.completeFeeBy(signer, 1000n);

// HARD SAFETY: exactly one input, and it is the pinned change cell.
const ins = tx.inputs.map((i) => `${i.previousOutput.txHash}:${BigInt(i.previousOutput.index)}`);
console.log("inputs:", ins);
const ok =
  tx.inputs.length === 1 &&
  tx.inputs[0].previousOutput.txHash === CHANGE_CELL.txHash &&
  BigInt(tx.inputs[0].previousOutput.index) === BigInt(CHANGE_CELL.index);
if (!ok) {
  console.error("ABORT: unexpected inputs — refusing to risk a code cell.");
  process.exit(1);
}
for (const o of tx.outputs) {
  console.log("  out", ccc.fixedPointToString(o.capacity), "CKB ->", o.lock.codeHash.slice(0, 10), o.lock.hashType);
}

if (SEND) {
  const hash = await signer.sendTransaction(tx);
  console.log("\nBROADCAST top-up tx:", hash);
  console.log("explorer: https://testnet.explorer.nervos.org/transaction/" + hash);
} else {
  console.log("\nDRY RUN ok — re-run with `send` to broadcast.");
}
