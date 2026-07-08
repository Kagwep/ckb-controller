// STEP 2 — genesis: create the empty game cell on testnet.
//
// Creates a cell with type = the deployed game type script (args = GAME_ID),
// locked by the testnet key (the operator), with EMPTY state data. The type script
// runs on this tx (0 inputs -> 1 output = genesis) and requires an empty state, so
// a successful broadcast already proves the deployed script works. Writes
// game-cell.json { txHash, index, capacity }.
//
// usage: node game-genesis.mjs [send]
import { ccc } from "@ckb-ccc/core";
import { readFile, writeFile } from "node:fs/promises";
import {
  GAME_ID, HT_DATA2, DEPLOY_FILE, CELL_FILE, makeSigner,
  selectPlainCells, assertOnlyInputs, explorer,
} from "./game-config.mjs";

const SEND = process.argv.includes("send");
const GAME_CELL_CKB = Number(process.env.GAME_CELL_CKB ?? "500"); // funds many transitions (each shrinks it a hair for fee)

const { codeHash, dep } = JSON.parse(await readFile(DEPLOY_FILE, "utf8"));
const { client, signer } = await makeSigner();
const lock = (await signer.getRecommendedAddressObj()).script;

const type = ccc.Script.from({ codeHash, hashType: "data2", args: GAME_ID });
void HT_DATA2; // (hashType string "data2" == 0x04)

const capacity = ccc.fixedPointFrom(GAME_CELL_CKB);
const headroom = ccc.fixedPointFrom(200);
const { inputs, total } = await selectPlainCells(client, lock, capacity + headroom);

const tx = ccc.Transaction.from({
  outputs: [{ lock, type, capacity }],
  outputsData: ["0x"], // empty state == genesis
});
for (const c of inputs) tx.inputs.push(ccc.CellInput.from({ previousOutput: c.outPoint, since: 0n }));
// The type script executes on genesis -> its code cell must be a cell dep.
tx.cellDeps.push(ccc.CellDep.from({ outPoint: dep, depType: "code" }));
await tx.completeFeeBy(signer, 1000n);
assertOnlyInputs(tx, inputs.map((c) => c.outPoint));

console.log(`game_id: ${GAME_ID}`);
console.log(`type code_hash: ${codeHash} (data2)`);
console.log(`game cell: ${GAME_CELL_CKB} CKB, empty state (output 0)`);
console.log(`plain inputs: ${inputs.length}, total ${total / 100000000n} CKB`);

if (SEND) {
  const hash = await signer.sendTransaction(tx);
  const cell = { txHash: hash, index: "0x0", capacity: capacity.toString() };
  await writeFile(CELL_FILE, JSON.stringify(cell, null, 2));
  console.log(`\nBROADCAST genesis: ${hash}`);
  console.log(`wrote ${CELL_FILE.pathname}`);
  console.log(`explorer: ${explorer(hash)}`);
} else {
  console.log("\nDRY RUN — re-run with `send` to broadcast.");
}
