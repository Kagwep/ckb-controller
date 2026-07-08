// Loader for the project config + deployment manifest (Node drivers).
//
//   ../controller.config.json      — settings (network, keys, session policy, …)
//   ../.controller/manifest.json   — deployed on-chain artifacts, keyed by network
//
// Env vars override individual values (NETWORK, RPC, KEYFILE, GAME_ID), so the
// old `RPC=… node script.mjs` invocations keep working. The browser side reads
// the same two JSON files via Vite imports (see src/config.ts); this file is the
// Node analog. Wire formats: ../docs/internals/wire-formats.md.
import { readFile, writeFile } from "node:fs/promises";
import { ccc } from "@ckb-ccc/core";

const ROOT = new URL("../", import.meta.url);
const MANIFEST_URL = new URL(".controller/manifest.json", ROOT);

export const CONFIG = JSON.parse(await readFile(new URL("controller.config.json", ROOT), "utf8"));
export const MANIFEST = JSON.parse(await readFile(MANIFEST_URL, "utf8"));

export const NETWORK = process.env.NETWORK ?? CONFIG.network;
export const NET = MANIFEST[NETWORK] ?? {};
export const RPC = process.env.RPC ?? CONFIG.networks[NETWORK].rpc;
export const KEYFILE = process.env.KEYFILE ?? CONFIG.keyFile;
export const GAME_ID = process.env.GAME_ID ?? CONFIG.gameId;

// Deployed code cells (throw with a pointer if the network isn't deployed yet).
function art(name) {
  const a = NET[name];
  if (!a) throw new Error(`manifest has no "${name}" for network "${NETWORK}" — deploy first (see .controller/manifest.json)`);
  return a;
}
export const LOCK = () => art("lock"); // { codeHash, hashType, dep }
export const AUTH_DEP = () => art("auth").dep;
export const SECP_DEP = () => art("secp256k1Sighash").dep;
export const GAME = () => art("game");

export const explorer = (h) => `${CONFIG.networks[NETWORK].explorerTx}${h}`;

/** Update one artifact in the manifest (used by deploy scripts after `send`). */
export async function updateManifest(name, value) {
  MANIFEST[NETWORK] = { ...(MANIFEST[NETWORK] ?? {}), [name]: value };
  await writeFile(MANIFEST_URL, JSON.stringify(MANIFEST, null, 2) + "\n");
}

/** A client + private-key signer for the configured key on the configured RPC. */
export async function makeSigner() {
  const priv = (await readFile(KEYFILE, "utf8")).trim();
  const client = new ccc.ClientPublicTestnet({ url: RPC });
  const signer = new ccc.SignerCkbPrivateKey(client, "0x" + priv.replace(/^0x/, ""));
  return { client, signer };
}

/** Load the wasm package in Node (web-target init needs the bytes passed in). */
export async function initWasm() {
  const mod = await import("./pkg/controller.js");
  await mod.default({
    module_or_path: await readFile(new URL("./pkg/controller_bg.wasm", import.meta.url)),
  });
  return mod;
}

const hexBytes = (h) => Uint8Array.from(h.replace(/^0x/, "").match(/.{2}/g).map((b) => parseInt(b, 16)));
export const OWNER_PRIV = hexBytes(CONFIG.session.ownerPrivkey);
export const SESSION_PRIV = hexBytes(CONFIG.session.sessionPrivkey);

/**
 * Reconstruct the controller account lock from config (same derivation as
 * src/deployed.ts): registered args = owner_hash ‖ session_params, with the
 * session policy from config. Returns { lock (ccc.Script), wasm } — byte-identical
 * to the deployed account cell's lock.
 */
export async function accountLock() {
  const wasm = await initWasm();
  const { blake2b } = await import("@noble/hashes/blake2b");
  const { secp256k1 } = await import("@noble/curves/secp256k1");
  const { utf8ToBytes, bytesToHex } = await import("@noble/hashes/utils");
  const personalization = utf8ToBytes("ckb-default-hash");
  const pubHash = (priv) =>
    "0x" + bytesToHex(blake2b(secp256k1.getPublicKey(priv, true), { dkLen: 32, personalization }).slice(0, 20));

  const s = CONFIG.session;
  const expires = s.expiresAt === "never" ? wasm.no_expiry() : BigInt(s.expiresAt);
  const root = s.policiesRoot === "wildcard" ? wasm.wildcard_root() : s.policiesRoot;
  const capShannons = (BigInt(s.spendCapCkb) * 100000000n).toString();
  const guardian = s.guardian ?? "0x" + "00".repeat(20);

  const params = wasm.session_params(pubHash(SESSION_PRIV), expires, root, capShannons, guardian);
  const args = wasm.registered_args(pubHash(OWNER_PRIV), params);
  const { codeHash, hashType } = LOCK();
  return { lock: ccc.Script.from({ codeHash, hashType, args }), wasm };
}
