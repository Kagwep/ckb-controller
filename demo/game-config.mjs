// Shared config for the multiplayer (aggregator + game cell) testnet bring-up.
//
// Flow: game-deploy.mjs (deploy the type script code cell) -> game-genesis.mjs
// (create the empty game cell) -> game-advance.mjs (commit a real session-signed
// transition). Deploy results go to the shared manifest (.controller/manifest.json)
// plus the legacy game-deploy.json / game-cell.json step files.
//
// Settings + deployed artifacts come from controller.config.json / the manifest
// via controller-config.mjs (env vars RPC / KEYFILE / GAME_ID still override).

import {
  RPC,
  KEYFILE,
  GAME_ID,
  AUTH_DEP as authDep,
  makeSigner,
  initWasm,
  explorer,
} from "./controller-config.mjs";

export { RPC, KEYFILE, GAME_ID, makeSigner, initWasm, explorer };

// The ckb-auth code cell (shared with the controller lock; the game type script
// spawns the SAME ckb-auth binary to verify intent signatures).
export const AUTH_DEP = authDep();

export const HT_DATA2 = 0x04; // CKB-VM v2, like the controller lock

export const GAME_BIN = new URL("../build/release/controller-game-cell", import.meta.url);
export const DEPLOY_FILE = new URL("./game-deploy.json", import.meta.url);
export const CELL_FILE = new URL("./game-cell.json", import.meta.url);

const CKB = 100_000_000n;

/**
 * Select ONLY plain change cells (dataLen 0, no type) under `lock` covering
 * `needShannons`, returning { inputs, total }. This is the hard-safety rule from
 * fund.mjs/topup.mjs: the testnet key also holds the deployed CODE cells (95k/151k/
 * 68k CKB) as live deps — a naive input scan could spend one and destroy a dep.
 * Never selects a cell with data or a type script.
 */
export async function selectPlainCells(client, lock, needShannons) {
  const inputs = [];
  let total = 0n;
  for await (const cell of client.findCellsByLock(lock, null, true)) {
    if (cell.outputData !== "0x") continue; // has data -> possibly a code cell
    if (cell.cellOutput.type) continue; // has a type -> not a plain cell
    inputs.push(cell);
    total += cell.cellOutput.capacity;
    if (total >= needShannons) break;
  }
  if (total < needShannons) {
    throw new Error(
      `insufficient plain balance: need ${needShannons / CKB} CKB, found ${total / CKB} CKB in plain cells. ` +
        `Fund the key (faucet.nervos.org) — deploying the ${68} KB type script costs ~68k CKB.`,
    );
  }
  return { inputs, total };
}

/** Assert every input in `tx` is one of `allowed` outpoints (no code cell slipped in). */
export function assertOnlyInputs(tx, allowed) {
  const set = new Set(allowed.map((o) => `${o.txHash}:${BigInt(o.index)}`));
  for (const i of tx.inputs) {
    const key = `${i.previousOutput.txHash}:${BigInt(i.previousOutput.index)}`;
    if (!set.has(key)) throw new Error(`ABORT: unexpected input ${key} — refusing to risk a code cell`);
  }
}
