// Drain a PER-USER account back to a single cell (Phase 3 runbook). A settle
// leaves the account with 2 cells (funding change + settle return); the lock
// allows only ONE account input per tx (MultipleInputs), so the next channel
// open fails until the account is a single cell again. Same session-signed
// drain as drain-account.mjs, but for a per-user identity whose keys live in
// a browser profile's localStorage — read via puppeteer (needs the vite dev
// server running at DEMO_URL so the origin matches userKeys.ts).
// Drained value goes to the configured sighash key (where funding came from).
// usage: node drain-user.mjs <profileDir> [send]   env: DEMO_URL, EDGE_PATH, KEYFILE
import { ccc } from "@ckb-ccc/core";
import { secp256k1 } from "@noble/curves/secp256k1";
import puppeteer from "puppeteer-core";
import { makeSigner, initWasm, LOCK, AUTH_DEP, explorer } from "./controller-config.mjs";
import { CONFIG } from "./controller-config.mjs";

const PROFILE = process.argv[2];
const SEND = process.argv.includes("send");
const BASE = process.env.DEMO_URL ?? "http://localhost:5173";
const EDGE = process.env.EDGE_PATH ?? "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const FEE = 100000n; // 0.001 CKB (a flat 1 CKB on a tiny tx exceeds the fee-rate cap)
if (!PROFILE) throw new Error("usage: node drain-user.mjs <profileDir> [send]");

// --- read the profile's persisted keys (userKeys.ts localStorage, origin-scoped)
const browser = await puppeteer.launch({
  executablePath: EDGE,
  headless: "new",
  userDataDir: PROFILE,
  args: ["--no-first-run", "--disable-features=msEdgeIdentityFeatures"],
});
let stored;
try {
  const page = await browser.newPage();
  await page.goto(`${BASE}/?multi=1`, { waitUntil: "domcontentloaded" });
  stored = await page.evaluate(() => localStorage.getItem("ckb-controller.userKeys.v1"));
} finally {
  await browser.close();
}
if (!stored) throw new Error(`profile has no persisted user keys (${PROFILE} · origin ${BASE})`);
const keys = JSON.parse(stored);
const hexBytes = (h) => Uint8Array.from(h.replace(/^0x/, "").match(/.{2}/g).map((b) => parseInt(b, 16)));
const SESSION_PRIV = hexBytes(keys.session);

// --- derive THIS user's account lock (same derivation as accountLock(), user keys)
const wasm = await initWasm();
const { blake2b } = await import("@noble/hashes/blake2b");
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
const args = wasm.registered_args(pubHash(hexBytes(keys.owner)), params);
const { codeHash, hashType } = LOCK();
const accLock = ccc.Script.from({ codeHash, hashType, args });

// --- find the account's cells; drain the SMALLEST, keep the largest
const { client, signer } = await makeSigner();
const cells = [];
for await (const cell of client.findCellsByLock(accLock, null, true)) cells.push(cell);
for (const c of cells)
  console.log(`cell: ${c.outPoint.txHash}:${BigInt(c.outPoint.index)} (${c.cellOutput.capacity / 100000000n} CKB)`);
if (cells.length === 0) throw new Error("no account cell found — wrong profile/origin?");
if (cells.length === 1) {
  console.log("account is already a single cell — nothing to drain.");
  process.exit(0);
}
const small = cells.reduce((m, c) => (c.cellOutput.capacity < m.cellOutput.capacity ? c : m));
const cap = small.cellOutput.capacity;
console.log(`draining: ${small.outPoint.txHash}:${BigInt(small.outPoint.index)} (${cap / 100000000n} CKB)`);

const destLock = (await signer.getRecommendedAddressObj()).script;
const tx = ccc.Transaction.from({
  inputs: [{ previousOutput: small.outPoint, since: 0n }],
  cellDeps: [
    { outPoint: LOCK().dep, depType: "code" },
    { outPoint: AUTH_DEP(), depType: "code" },
  ],
  outputs: [{ lock: destLock, capacity: cap - FEE }],
  outputsData: ["0x"],
  witnesses: ["0x"],
});

// controller message = blake2b(raw tx, cell_deps cleared); the USER's session signs
const msg = wasm.tx_message(ccc.hexFrom(tx.toBytes()));
const sig0 = secp256k1.sign(msg.slice(2), SESSION_PRIV);
const sig = "0x" + Buffer.from([...sig0.toCompactRawBytes(), sig0.recovery]).toString("hex");
tx.witnesses = [wasm.session_witness_registered(sig, "", wasm.channel_proof_region())];

console.log(`  -> out ${(cap - FEE) / 100000000n} CKB to ${destLock.codeHash.slice(0, 10)} (sighash)`);
if (SEND) {
  const h = await client.sendTransaction(tx);
  console.log("\nBROADCAST:", h, "\nexplorer: " + explorer(h));
} else {
  console.log("\nDRY RUN — add `send` to broadcast.");
}
