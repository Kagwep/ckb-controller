// STEP 3 — advance the game cell: commit a real session-signed transition.
//
// Reads the current game cell, builds an intent batch from demo player session
// keys (signing the SAME message the browser signs), computes the next state with
// the wasm `game_apply` (byte-exact with the on-chain rule), and commits the
// transition tx. The node runs the deployed type script, which re-derives the
// transition and verifies every intent signature via ckb-auth — so a successful
// broadcast proves the aggregator end-to-end on testnet.
//
// The game cell self-funds each transition by shrinking a hair (0.001 CKB fee), so
// the only input is the game cell itself — no fee-cell selection, no code-cell risk.
//
// usage: node game-advance.mjs [send]
import { ccc } from "@ckb-ccc/core";
import { secp256k1 } from "@noble/curves/secp256k1";
import { blake2b } from "@noble/hashes/blake2b";
import { bytesToHex, hexToBytes, utf8ToBytes } from "@noble/hashes/utils";
import { readFile, writeFile } from "node:fs/promises";
import { GAME_ID, AUTH_DEP, DEPLOY_FILE, CELL_FILE, makeSigner, initWasm, explorer } from "./game-config.mjs";

const SEND = process.argv.includes("send");
const POINTS = BigInt(process.env.POINTS ?? "5");
const FEE = 100_000n; // 0.001 CKB — the game cell shrinks by this each transition

// CKB hash (blake2b-256 personalised "ckb-default-hash") -> 20-byte pubkey hash.
const CKB_HASH_PERSONAL = utf8ToBytes("ckb-default-hash");
const hx = (b) => "0x" + bytesToHex(b);
const strip = (h) => (h.startsWith("0x") ? h.slice(2) : h);
const pubHash = (priv) => hx(blake2b(secp256k1.getPublicKey(priv, true), { dkLen: 32, personalization: CKB_HASH_PERSONAL }).slice(0, 20));
function signRecoverable(msgHex, priv) {
  const sig = secp256k1.sign(hexToBytes(strip(msgHex)), priv);
  const out = new Uint8Array(65);
  out.set(sig.toCompactRawBytes(), 0);
  out[64] = sig.recovery;
  return hx(out);
}

// Deterministic demo players (so repeated runs accumulate for the same two).
const PLAYERS = [new Uint8Array(32).fill(0x41), new Uint8Array(32).fill(0x42)];

const wasm = await initWasm();
const { codeHash, dep } = JSON.parse(await readFile(DEPLOY_FILE, "utf8"));
const { client, signer } = await makeSigner();

// Current game cell, located LIVE by its type script (same as the operator at
// startup) — game-cell.json goes stale after every transition, so it's only a
// fallback hint, not the source of truth.
const typeScript = ccc.Script.from({ codeHash, hashType: "data2", args: GAME_ID });
let cell = null;
for await (const c of client.findCellsByType(typeScript, true)) {
  cell = c;
  break;
}
if (!cell) throw new Error(`no live game cell with type ${codeHash} / game ${GAME_ID} — run game-genesis.mjs`);
const outPoint = cell.outPoint;
console.log(`game cell: ${outPoint.txHash}:${BigInt(outPoint.index)}`);
const currentData = cell.outputData;
const board = JSON.parse(wasm.game_decode_state(currentData));
const nonceOf = (h) => {
  const p = board.players.find((x) => x.hash.toLowerCase() === h.toLowerCase());
  return p ? BigInt(p.nonce) : 0n;
};

// Build one session-signed intent per demo player (nonce = their prev + 1).
const intents = PLAYERS.map((priv) => {
  const hash = pubHash(priv);
  const nonce = nonceOf(hash) + 1n;
  const msg = wasm.game_intent_message(GAME_ID, hash, POINTS, nonce);
  const sig = signRecoverable(msg, priv);
  console.log(`  intent: player ${hash.slice(0, 12)}… +${POINTS} nonce ${nonce}`);
  return wasm.game_encode_intent(hash, POINTS, nonce, sig);
});
const batch = wasm.game_encode_batch(intents);
const nextData = wasm.game_apply(currentData, batch);

console.log(`current seq ${board.seq}, ${board.players.length} player(s)`);
console.log(`next state: ${JSON.parse(wasm.game_decode_state(nextData)).seq} seq`);

// Transition tx: the single game cell input -> the next game cell (shrunk by FEE),
// batch in the input's witness input_type.
const nextCap = cell.cellOutput.capacity - FEE;
const tx = ccc.Transaction.from({
  inputs: [{ previousOutput: outPoint }],
  outputs: [{ lock: cell.cellOutput.lock, type: cell.cellOutput.type, capacity: nextCap }],
  outputsData: [nextData],
});
tx.setWitnessArgsAt(0, ccc.WitnessArgs.from({ inputType: batch }));
// Deps: the game type-script code cell + the ckb-auth code cell (spawned per intent).
tx.cellDeps.push(ccc.CellDep.from({ outPoint: dep, depType: "code" }));
tx.cellDeps.push(ccc.CellDep.from({ outPoint: AUTH_DEP, depType: "code" }));

if (SEND) {
  const hash = await signer.sendTransaction(tx); // signs the game cell lock (operator key)
  await writeFile(CELL_FILE, JSON.stringify({ txHash: hash, index: "0x0", capacity: nextCap.toString() }, null, 2));
  console.log(`\nBROADCAST transition: ${hash}`);
  console.log(`game cell -> ${hash}:0  (${nextCap / 100000000n} CKB)`);
  console.log(`explorer: ${explorer(hash)}`);
} else {
  console.log(`\nDRY RUN — batch ${((batch.length - 2) / 2 / 101).toFixed(0)} intent(s), fee ${FEE} shannons. Re-run with \`send\`.`);
}
