// STEP 1 — deploy the game type-script code cell to testnet.
//
// Creates a cell whose data is the compiled type script (build/release/
// controller-game-cell), locked by the testnet key. Its DATA HASH becomes the
// game type script's code_hash. Writes game-deploy.json { codeHash, dep }.
//
// usage: node game-deploy.mjs [send]   (dry-run without `send`)
// COST: ~68k CKB (1 CKB/byte on-chain) — the key must be funded (faucet.nervos.org).
import { ccc } from "@ckb-ccc/core";
import { readFile, writeFile } from "node:fs/promises";
import { GAME_BIN, DEPLOY_FILE, makeSigner, selectPlainCells, assertOnlyInputs, explorer } from "./game-config.mjs";
import { updateManifest } from "./controller-config.mjs";

const SEND = process.argv.includes("send");

const code = new Uint8Array(await readFile(GAME_BIN));
const codeHash = ccc.hashCkb(code); // data hash = the type script's code_hash
const { client, signer } = await makeSigner();
const lock = (await signer.getRecommendedAddressObj()).script;

// Cell capacity for a data cell ≈ data length (1 CKB/byte) + script/overhead.
const capacity = ccc.fixedPointFrom(code.length + 200);
const headroom = ccc.fixedPointFrom(500); // fee + change slack

const { inputs, total } = await selectPlainCells(client, lock, capacity + headroom);

const tx = ccc.Transaction.from({ outputs: [{ lock, capacity }], outputsData: [ccc.hexFrom(code)] });
for (const c of inputs) tx.inputs.push(ccc.CellInput.from({ previousOutput: c.outPoint, since: 0n }));
await tx.completeFeeBy(signer, 1000n); // adds change; must not pull new inputs
assertOnlyInputs(tx, inputs.map((c) => c.outPoint)); // never a code cell

console.log(`game type script: ${code.length} bytes`);
console.log(`code_hash (data hash): ${codeHash}`);
console.log(`plain inputs: ${inputs.length}, total ${total / 100000000n} CKB`);
console.log(`code cell capacity: ${ccc.fixedPointToString(capacity)} CKB (output 0)`);

if (SEND) {
  const hash = await signer.sendTransaction(tx);
  const dep = { codeHash, dep: { txHash: hash, index: "0x0" } };
  await writeFile(DEPLOY_FILE, JSON.stringify(dep, null, 2));
  await updateManifest("game", { codeHash, hashType: "data2", dep: dep.dep, depType: "code" });
  console.log(`\nBROADCAST deploy: ${hash}`);
  console.log(`wrote ${DEPLOY_FILE.pathname} + .controller/manifest.json`);
  console.log(`explorer: ${explorer(hash)}`);
} else {
  console.log("\nDRY RUN — re-run with `send` to broadcast.");
}
