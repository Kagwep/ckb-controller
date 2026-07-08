// Live-test client for the game-operator: acts exactly like a game.html tab —
// fresh session key, wasm intent message, @noble recoverable sig, POST /intent.
// usage: node post-intent.mjs [operatorUrl] [points] [forge]
//   forge: send a zeroed signature — must be admitted by the sequencer (sigs are
//   checked on-chain only), rejected by the node, and shed from the queue.
import { secp256k1 } from "@noble/curves/secp256k1";
import { blake2b } from "@noble/hashes/blake2b";
import { bytesToHex, hexToBytes, utf8ToBytes } from "@noble/hashes/utils";
import { GAME_ID, initWasm, explorer } from "./game-config.mjs";
import { CONFIG } from "./controller-config.mjs";

const OPERATOR = (process.argv[2] ?? `http://${CONFIG.operator.listen}`).replace(/\/$/, "");
const POINTS = BigInt(process.argv[3] ?? "5");

const CKB_HASH_PERSONAL = utf8ToBytes("ckb-default-hash");
const hx = (b) => "0x" + bytesToHex(b);
const strip = (h) => (h.startsWith("0x") ? h.slice(2) : h);

const wasm = await initWasm();

const priv = secp256k1.utils.randomPrivateKey();
const playerHash = hx(
  blake2b(secp256k1.getPublicKey(priv, true), { dkLen: 32, personalization: CKB_HASH_PERSONAL }).slice(0, 20),
);
const nonce = 1n; // fresh player

const FORGE = process.argv.includes("forge");
const msg = wasm.game_intent_message(GAME_ID, playerHash, POINTS, nonce);
const sig = secp256k1.sign(hexToBytes(strip(msg)), priv);
const sig65 = new Uint8Array(65);
if (!FORGE) {
  sig65.set(sig.toCompactRawBytes(), 0);
  sig65[64] = sig.recovery;
}
const intent = wasm.game_encode_intent(playerHash, POINTS, nonce, hx(sig65));
if (FORGE) console.log("(forged signature — expecting on-chain rejection)");

console.log(`player ${playerHash} +${POINTS} nonce ${nonce} -> ${OPERATOR}/intent`);
const res = await fetch(`${OPERATOR}/intent`, {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ intent }),
});
const body = await res.json();
console.log(res.status, JSON.stringify(body));
if (res.ok) {
  console.log(`explorer: ${explorer(body.tx_hash)}`);
  const board = await (await fetch(`${OPERATOR}/game`)).json();
  console.log("board:", JSON.stringify(board));
}
