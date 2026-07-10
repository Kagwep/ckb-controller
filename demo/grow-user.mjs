// GROW a PER-USER account cell in place (capacity top-up WITHOUT creating a
// second account cell — fund-user.mjs correctly refuses an address that already
// has one; this is the grow path for it). Same tx shape as grow-account.mjs
// (account input session-signed + one plain sighash input → single bigger
// account cell + change), but for a per-user identity whose keys live in a
// browser profile's localStorage — read via puppeteer (vite must be running at
// DEMO_URL so the origin matches userKeys.ts).
// usage: node grow-user.mjs <profileDir> [growCkb] [send]   env: DEMO_URL, EDGE_PATH, KEYFILE
import { ccc } from "@ckb-ccc/core";
import { secp256k1 } from "@noble/curves/secp256k1";
import puppeteer from "puppeteer-core";
import { makeSigner, initWasm, LOCK, AUTH_DEP, explorer, CONFIG } from "./controller-config.mjs";

const PROFILE = process.argv[2];
const args = process.argv.slice(3).filter((a) => a !== "send");
const GROW = ccc.fixedPointFrom(args[0] && !isNaN(Number(args[0])) ? args[0] : "500");
const SEND = process.argv.includes("send");
const BASE = process.env.DEMO_URL ?? "http://localhost:5173";
const EDGE = process.env.EDGE_PATH ?? "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const FEE = 100000n; // 0.001 CKB
if (!PROFILE) throw new Error("usage: node grow-user.mjs <profileDir> [growCkb] [send]");

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
const lockArgs = wasm.registered_args(pubHash(hexBytes(keys.owner)), params);
const { codeHash, hashType } = LOCK();
const accLock = ccc.Script.from({ codeHash, hashType, args: lockArgs });

// --- the single account cell (grow keeps the single-cell invariant)
const { client, signer } = await makeSigner();
let account = null,
  count = 0;
for await (const cell of client.findCellsByLock(accLock, null, true)) {
  account = cell;
  count++;
}
if (!account) throw new Error("no account cell — fund-user.mjs first");
if (count !== 1) throw new Error(`account must be a single cell (found ${count}) — run drain-user.mjs first`);
const accCap = account.cellOutput.capacity;
console.log(`account: ${account.outPoint.txHash}:${BigInt(account.outPoint.index)} = ${accCap / 100000000n} CKB`);

// one plain sighash cell covering GROW + change occupied (61) + fee
const sighashLock = (await signer.getRecommendedAddressObj()).script;
const need = GROW + ccc.fixedPointFrom(62) + FEE;
let feeCell = null;
for await (const cell of client.findCellsByLock(sighashLock, null, true)) {
  if (cell.outputData === "0x" && !cell.cellOutput.type && cell.cellOutput.capacity >= need) {
    feeCell = cell;
    break;
  }
}
if (!feeCell) throw new Error(`no plain sighash cell with ${need / 100000000n} CKB`);
console.log(`sighash in: ${feeCell.outPoint.txHash}:${BigInt(feeCell.outPoint.index)} = ${feeCell.cellOutput.capacity / 100000000n} CKB`);

const tx = ccc.Transaction.from({
  inputs: [
    { previousOutput: account.outPoint, since: 0n },
    { previousOutput: feeCell.outPoint, since: 0n },
  ],
  cellDeps: [
    { outPoint: LOCK().dep, depType: "code" },
    { outPoint: AUTH_DEP(), depType: "code" },
  ],
  outputs: [
    { lock: accLock, capacity: accCap + GROW }, // the grown account (output 0)
    { lock: sighashLock, capacity: feeCell.cellOutput.capacity - GROW - FEE }, // change
  ],
  outputsData: ["0x", "0x"],
  witnesses: ["0x", "0x"],
});

// session witness for input 0: msg = blake2b(raw tx, cell_deps cleared) — witnesses
// are NOT covered, so CCC filling witness 1 later doesn't invalidate it.
const msg = wasm.tx_message(ccc.hexFrom(tx.toBytes()));
const sg = secp256k1.sign(msg.slice(2), SESSION_PRIV);
const sig = "0x" + Buffer.from([...sg.toCompactRawBytes(), sg.recovery]).toString("hex");
tx.witnesses[0] = wasm.session_witness_registered(sig, "", wasm.channel_proof_region());

console.log(`  -> account ${(accCap + GROW) / 100000000n} CKB, change ${(feeCell.cellOutput.capacity - GROW - FEE) / 100000000n} CKB`);

if (SEND) {
  // CCC signs only its sighash group (input 1) and adds the secp dep group.
  const hash = await signer.sendTransaction(tx);
  console.log("\nBROADCAST grow:", hash, "\nexplorer: " + explorer(hash));
} else {
  console.log("\nDRY RUN — add `send` to broadcast.");
}
