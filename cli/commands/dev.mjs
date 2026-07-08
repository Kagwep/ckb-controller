// dev — one-shot local stack: boot a CKB dev chain, deploy everything, start
// the game operator (and the demo if present), print the play URL.
//
//   chain    ckb v0.2xx dev chain (+ indexer + miner) under .controller/devnet-chain,
//            CKB2023 active at epoch 0 (data2/VM2 — the lock needs `spawn`),
//            cellbase_maturity 0, IntegrationTest module on (instant blocks
//            during bring-up via generate_block).
//   funds    the well-known dev key (funded billions in the dev genesis) — written
//            to .controller/devkey.txt; it deploys and runs the operator.
//   deploys  same ensureDeployed core as `deploy`, against the local RPC with a
//            CCC script registry pointed at the DEVNET secp dep group (discovered
//            from genesis block 0 — CCC's default registry is testnet-only).
//   servers  game-operator (CHAIN=http, NETWORK=devnet) + `npm run dev` in demo/.
//
// Needs the ckb + ckb-cli release binaries: CKBDIR env or networks.devnet.ckbDir.
// Ctrl-C stops everything (children die with this process).
import { ccc } from "@ckb-ccc/core";
import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile, open } from "node:fs/promises";
import { join } from "node:path";
import { loadCtx, saveArtifact } from "../lib/config.mjs";
import { ensureDeployed } from "./deploy.mjs";

// The classic CKB dev-chain key (issued funds in every `ckb init -c dev` genesis).
const DEV_PRIVKEY = "d00c06bfd800d27397002dca6fb0993d5ba6399b4238b2f29ee9deb97593d2bc";
const DEV_BA_ARG = "0xc8328aabcd9b9e8e64fbc566c4385c3bdeb219d7"; // its blake160

async function rpc(url, method, params = []) {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ id: 1, jsonrpc: "2.0", method, params }),
    signal: AbortSignal.timeout(5000),
  });
  const body = await res.json();
  if (body.error) throw new Error(`${method}: ${JSON.stringify(body.error)}`);
  return body.result;
}

async function patchFile(path, patches) {
  let text = await readFile(path, "utf8");
  for (const [test, apply] of patches) if (!test(text)) text = apply(text);
  await writeFile(path, text);
}

export async function run(args, { configPath }) {
  process.env.NETWORK = "devnet";
  const ctx = await loadCtx(configPath);
  const rpcUrl = ctx.rpc; // networks.devnet.rpc, default http://127.0.0.1:8114

  const ckbDir = process.env.CKBDIR ?? ctx.config.networks?.devnet?.ckbDir;
  if (!ckbDir) throw new Error("set networks.devnet.ckbDir in controller.config.json (or CKBDIR env) to the ckb release dir");
  const exe = process.platform === "win32" ? ".exe" : "";
  const ckb = join(ckbDir, `ckb${exe}`);
  if (!existsSync(ckb)) throw new Error(`ckb binary not found at ${ckb}`);

  const stateDir = join(ctx.root, ".controller");
  const dataDir = join(stateDir, "devnet-chain");
  await mkdir(stateDir, { recursive: true });
  const devkeyPath = join(stateDir, "devkey.txt");
  await writeFile(devkeyPath, DEV_PRIVKEY + "\n");

  // ── 1. chain data dir (init once, then reuse — chain state persists) ───────
  if (!existsSync(join(dataDir, "specs", "dev.toml"))) {
    console.log("initializing dev chain…");
    const init = spawnSync(ckb, ["init", "-C", dataDir, "-c", "dev", "--force", "--ba-arg", DEV_BA_ARG], { encoding: "utf8" });
    if (init.status !== 0) throw new Error(`ckb init failed: ${init.stderr}`);
    // CKB2023 at epoch 0 (VM2/data2 — the lock uses `spawn`), instant cellbase.
    await patchFile(join(dataDir, "specs", "dev.toml"), [
      [(t) => /cellbase_maturity\s*=\s*0\b/.test(t), (t) => t.replace(/\[params\]/, "[params]\ncellbase_maturity = 0")],
      [(t) => /ckb2023\s*=\s*0\b/.test(t), (t) => t + "\n[params.hardfork]\nckb2023 = 0\n"],
    ]);
    // IntegrationTest RPC module -> generate_block for instant commits.
    await patchFile(join(dataDir, "ckb.toml"), [
      [(t) => t.includes("IntegrationTest"), (t) => t.replace(/"Debug"/, '"Debug", "IntegrationTest"')],
    ]);
  }

  // ── 2. node + miner ─────────────────────────────────────────────────────────
  const children = [];
  const daemon = async (name, cmd, cmdArgs, cwd) => {
    const log = await open(join(stateDir, `${name}.log`), "w");
    const child = spawn(cmd, cmdArgs, { cwd, stdio: ["ignore", log.fd, log.fd], env: { ...process.env } });
    children.push({ name, child });
    child.on("exit", (code) => code !== null && code !== 0 && console.log(`! ${name} exited with code ${code} (see .controller/${name}.log)`));
    return child;
  };
  const shutdown = () => {
    for (const { child } of children) try { child.kill(); } catch {}
  };
  process.on("SIGINT", () => { shutdown(); process.exit(0); });
  process.on("exit", shutdown);

  console.log("starting ckb (run --indexer) + miner…");
  await daemon("ckb", ckb, ["run", "-C", dataDir, "--indexer"]);
  for (let i = 0; ; i++) {
    try { await rpc(rpcUrl, "get_tip_block_number"); break; }
    catch { if (i > 30) throw new Error("ckb node did not come up — see .controller/ckb.log"); await new Promise((r) => setTimeout(r, 1000)); }
  }
  await daemon("miner", ckb, ["miner", "-C", dataDir]);
  const mine = async (n = 3) => { for (let i = 0; i < n; i++) await rpc(rpcUrl, "generate_block").catch(() => {}); };
  await mine(6); // mature the chain a little

  // ── 3. a CCC client that knows the DEVNET secp dep group ───────────────────
  const genesis = await rpc(rpcUrl, "get_block_by_number", ["0x0"]);
  const depGroupTx = genesis.transactions[1].hash;
  // record it — the operator's transition txs need this dep group too
  await saveArtifact(ctx, "secp256k1Sighash", {
    dep: { txHash: depGroupTx, index: "0x0" },
    depType: "depGroup",
  });
  const base = new ccc.ClientPublicTestnet();
  const scripts = {
    ...base.scripts,
    [ccc.KnownScript.Secp256k1Blake160]: {
      ...base.scripts[ccc.KnownScript.Secp256k1Blake160],
      cellDeps: [{ cellDep: { outPoint: { txHash: depGroupTx, index: 0 }, depType: "depGroup" } }],
    },
  };
  const client = new ccc.ClientPublicTestnet({ url: rpcUrl, scripts });
  const signer = new ccc.SignerCkbPrivateKey(client, "0x" + DEV_PRIVKEY);
  ctx.keyFile = devkeyPath;

  // wait for the genesis-issued dev funds to be indexed
  const keyLock = (await signer.getRecommendedAddressObj()).script;
  for (let i = 0; ; i++) {
    let total = 0n;
    for await (const c of client.findCellsByLock(keyLock, null, false)) { total += c.cellOutput.capacity; if (total > 400_000n * 100_000_000n) break; }
    if (total > 400_000n * 100_000_000n) break;
    if (i > 30) throw new Error("dev key never showed funds — indexer lagging? see .controller/ckb.log");
    await mine(2);
    await new Promise((r) => setTimeout(r, 1000));
  }

  // ── 4. deploy everything (same core as `deploy`) ───────────────────────────
  console.log(`deploying to devnet (${rpcUrl})…`);
  await ensureDeployed(ctx, client, signer, { send: true, onTick: () => mine(3) });

  // ── 5. operator + demo ──────────────────────────────────────────────────────
  const listen = ctx.config.operator?.listen ?? "127.0.0.1:9944";
  const opExe = join(ctx.root, "target", "debug", `game-operator${exe}`);
  const opEnv = { NETWORK: "devnet", CHAIN: "http", RPC: rpcUrl, KEYFILE: devkeyPath, CONTROLLER_CONFIG: ctx.configPath, LISTEN: listen };
  if (existsSync(opExe)) {
    console.log("starting game-operator…");
    const log = await open(join(stateDir, "operator.log"), "w");
    const child = spawn(opExe, [], { cwd: ctx.root, stdio: ["ignore", log.fd, log.fd], env: { ...process.env, ...opEnv } });
    children.push({ name: "operator", child });
  } else {
    console.log(`! game-operator binary not built — run: cargo build -p paymaster-service --bin game-operator`);
    console.log(`  then: ${Object.entries(opEnv).map(([k, v]) => `${k}=${v}`).join(" ")} ${opExe}`);
  }

  let demoUrl = null;
  if (existsSync(join(ctx.root, "demo", "package.json"))) {
    console.log("starting demo (vite)…");
    const npm = process.platform === "win32" ? "npm.cmd" : "npm";
    const viteLog = join(stateDir, "vite.log");
    const log = await open(viteLog, "w");
    const child = spawn(npm, ["run", "dev"], { cwd: join(ctx.root, "demo"), stdio: ["ignore", log.fd, log.fd], env: { ...process.env }, shell: process.platform === "win32" });
    children.push({ name: "vite", child });
    // vite picks the next free port — read the real one from its banner
    let port = null;
    for (let i = 0; i < 30 && !port; i++) {
      await new Promise((r) => setTimeout(r, 1000));
      port = (await readFile(viteLog, "utf8").catch(() => "")).match(/localhost:(\d+)/)?.[1] ?? null;
    }
    demoUrl = `http://localhost:${port ?? 5173}/game.html?operator=http://${listen}&game=${ctx.gameId}`;
  }

  // steady-state block production is the miner's job now; verify the operator.
  for (let i = 0; i < 30; i++) {
    try {
      const h = await (await fetch(`http://${listen}/health`, { signal: AbortSignal.timeout(2000) })).json();
      console.log(`operator up: seq ${h.seq}`);
      break;
    } catch { await new Promise((r) => setTimeout(r, 1000)); }
  }

  console.log("\n════════════════════ LOCAL STACK UP ════════════════════");
  console.log(` chain    : ${rpcUrl}  (logs: .controller/ckb.log, miner.log)`);
  console.log(` operator : http://${listen}`);
  if (demoUrl) console.log(` PLAY     : ${demoUrl}`);
  console.log(` Ctrl-C stops chain + miner + operator + demo.`);
  console.log("══════════════════════════════════════════════════════════");

  // keep the process (and children) alive until Ctrl-C
  await new Promise(() => {});
}
