// Config + manifest loading for the CLI.
//
// Resolution: --config <path> > CONTROLLER_CONFIG env > upward search from the
// cwd for controller.config.json. The manifest lives at
// <config dir>/.controller/manifest.json. Env vars (NETWORK, RPC, KEYFILE,
// GAME_ID) override individual values, same as demo/controller-config.mjs.
import { readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { ccc } from "@ckb-ccc/core";

export function findConfigPath(explicit) {
  if (explicit) return resolve(explicit);
  if (process.env.CONTROLLER_CONFIG) return resolve(process.env.CONTROLLER_CONFIG);
  let dir = process.cwd();
  for (;;) {
    const candidate = join(dir, "controller.config.json");
    if (existsSync(candidate)) return candidate;
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

/** Load config + manifest into one context object the commands share. */
export async function loadCtx(explicitConfig) {
  const configPath = findConfigPath(explicitConfig);
  if (!configPath) {
    throw new Error(
      "no controller.config.json found (searched upward from cwd) — run `ckb-controller init` or pass --config <path>",
    );
  }
  const root = dirname(configPath);
  const manifestPath = join(root, ".controller", "manifest.json");
  const config = JSON.parse(await readFile(configPath, "utf8"));
  const manifest = existsSync(manifestPath) ? JSON.parse(await readFile(manifestPath, "utf8")) : {};

  const network = process.env.NETWORK ?? config.network;
  const netCfg = config.networks?.[network] ?? {};
  const ctx = {
    configPath,
    manifestPath,
    root,
    config,
    manifest,
    network,
    net: manifest[network] ?? {},
    rpc: process.env.RPC ?? netCfg.rpc,
    keyFile: resolveKeyFile(process.env.KEYFILE ?? config.keyFile, root),
    gameId: process.env.GAME_ID ?? config.gameId,
    explorerTx: netCfg.explorerTx ?? "",
  };
  ctx.explorer = (h) => (ctx.explorerTx ? `${ctx.explorerTx}${h}` : h);
  return ctx;
}

/** Relative keyFile paths are relative to the config's directory, not the cwd. */
function resolveKeyFile(p, root) {
  if (!p) return p;
  return isAbsolute(p) ? p : resolve(root, p);
}

/** Write one artifact back into the manifest for the active network. */
export async function saveArtifact(ctx, name, value) {
  ctx.manifest[ctx.network] = { ...(ctx.manifest[ctx.network] ?? {}), [name]: value };
  ctx.net = ctx.manifest[ctx.network];
  await writeFile(ctx.manifestPath, JSON.stringify(ctx.manifest, null, 2) + "\n");
}

export async function makeSigner(ctx) {
  const priv = (await readFile(ctx.keyFile, "utf8")).trim();
  const client = new ccc.ClientPublicTestnet({ url: ctx.rpc });
  const signer = new ccc.SignerCkbPrivateKey(client, "0x" + priv.replace(/^0x/, ""));
  return { client, signer };
}

/** Load the wasm pkg: the project's own build if present (repo checkouts),
 *  else the copy bundled with this CLI package (standalone projects). */
export async function initWasm(ctx) {
  const cliPkg = fileURLToPath(new URL("../pkg", import.meta.url));
  for (const dir of [join(ctx.root, "wasm/pkg"), join(ctx.root, "demo/pkg"), cliPkg]) {
    const js = join(dir, "controller.js");
    if (!existsSync(js)) continue;
    const mod = await import(pathToFileURL(js).href);
    await mod.default({ module_or_path: await readFile(join(dir, "controller_bg.wasm")) });
    return mod;
  }
  throw new Error("wasm pkg not found — run ./scripts/build-wasm.sh from the repo root");
}

const hexBytes = (h) => Uint8Array.from(h.replace(/^0x/, "").match(/.{2}/g).map((b) => parseInt(b, 16)));

/** Session/owner keys + the derived account lock (same bytes as the demo/CLI). */
export async function accountLock(ctx) {
  const wasm = await initWasm(ctx);
  const { blake2b } = await import("@noble/hashes/blake2b");
  const { secp256k1 } = await import("@noble/curves/secp256k1");
  const { utf8ToBytes, bytesToHex } = await import("@noble/hashes/utils");
  const personalization = utf8ToBytes("ckb-default-hash");
  const pubHash = (priv) =>
    "0x" + bytesToHex(blake2b(secp256k1.getPublicKey(priv, true), { dkLen: 32, personalization }).slice(0, 20));

  const s = ctx.config.session;
  const ownerPriv = hexBytes(s.ownerPrivkey);
  const sessionPriv = hexBytes(s.sessionPrivkey);
  const expires = s.expiresAt === "never" ? wasm.no_expiry() : BigInt(s.expiresAt);
  const root = s.policiesRoot === "wildcard" ? wasm.wildcard_root() : s.policiesRoot;
  const capShannons = (BigInt(s.spendCapCkb) * 100000000n).toString();
  const guardian = s.guardian ?? "0x" + "00".repeat(20);

  const params = wasm.session_params(pubHash(sessionPriv), expires, root, capShannons, guardian);
  const args = wasm.registered_args(pubHash(ownerPriv), params);
  const lockArt = ctx.net.lock;
  if (!lockArt) throw new Error(`manifest has no "lock" for network "${ctx.network}" — run \`ckb-controller deploy\``);
  const address = wasm.controller_address(lockArt.codeHash, 0x04, args, true);
  return {
    lock: ccc.Script.from({ codeHash: lockArt.codeHash, hashType: lockArt.hashType, args }),
    address,
    ownerPriv,
    sessionPriv,
    wasm,
  };
}

// ---- shared safety helpers (same rules as demo/game-config.mjs) -------------

/** Plain cells only (no data, no type) — never risks a live code cell. */
export async function selectPlainCells(client, lock, needShannons) {
  const inputs = [];
  let total = 0n;
  for await (const cell of client.findCellsByLock(lock, null, true)) {
    if (cell.outputData !== "0x" || cell.cellOutput.type) continue;
    inputs.push(cell);
    total += cell.cellOutput.capacity;
    if (total >= needShannons) break;
  }
  if (total < needShannons) {
    throw new Error(
      `insufficient plain balance: need ${needShannons / 100000000n} CKB, found ${total / 100000000n} CKB — fund the key (https://faucet.nervos.org)`,
    );
  }
  return { inputs, total };
}

export function assertOnlyInputs(tx, allowed) {
  const set = new Set(allowed.map((o) => `${o.txHash}:${BigInt(o.index)}`));
  for (const i of tx.inputs) {
    const key = `${i.previousOutput.txHash}:${BigInt(i.previousOutput.index)}`;
    if (!set.has(key)) throw new Error(`ABORT: unexpected input ${key} — refusing to risk a code cell`);
  }
}

export const CKB = 100_000_000n;
export const fmtCkb = (shannons) => `${shannons / CKB} CKB`;
