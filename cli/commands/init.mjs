// init — scaffold a new controller project in <dir> (default: cwd):
//
//   controller.config.json      fresh game id + fresh owner/session/deploy keys
//   .controller/manifest.json   seeded with the SHARED testnet code cells, so
//                               `deploy --send` only creates the game cell +
//                               account (~1k CKB — one faucet claim)
//   testnet-key.txt             a new deploy/operator key (fund it!)
//   .gitignore                  keeps the key out of git
//
// Prints the funding address + next steps. Refuses to overwrite existing files.
import { ccc } from "@ckb-ccc/core";
import { randomBytes } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { KNOWN_DEPLOYMENTS } from "../lib/known-deployments.mjs";

const hex = (b) => "0x" + Buffer.from(b).toString("hex");

export async function run(args) {
  const dir = resolve(args[0] ?? ".");
  const cfgPath = join(dir, "controller.config.json");
  if (existsSync(cfgPath)) throw new Error(`${cfgPath} already exists`);
  await mkdir(join(dir, ".controller"), { recursive: true });

  const deployKey = randomBytes(32);
  const config = {
    "//": "Controller project config. Session keys are DEMO-grade (plain hex in a file) — swap for real key management before mainnet. Env vars (RPC, KEYFILE, GAME_ID, NETWORK) override.",
    network: "testnet",
    keyFile: "./testnet-key.txt",
    gameId: hex(randomBytes(32)),
    session: {
      ownerPrivkey: hex(randomBytes(32)),
      sessionPrivkey: hex(randomBytes(32)),
      spendCapCkb: 1000,
      expiresAt: "never",
      policiesRoot: "wildcard",
      guardian: null,
    },
    operator: { listen: "127.0.0.1:9944", chain: "http", feeShannons: 100000 },
    fiber: { rpc: "http://127.0.0.1:8227" },
    networks: {
      testnet: { rpc: "https://testnet.ckb.dev/rpc", explorerTx: "https://testnet.explorer.nervos.org/transaction/" },
      devnet: { rpc: "http://127.0.0.1:8114", explorerTx: "" },
    },
  };

  await writeFile(cfgPath, JSON.stringify(config, null, 2) + "\n");
  await writeFile(
    join(dir, ".controller", "manifest.json"),
    JSON.stringify({ "//": "Deployed artifacts per network. testnet is pre-seeded with the shared public code cells.", ...KNOWN_DEPLOYMENTS, devnet: {} }, null, 2) + "\n",
  );
  await writeFile(join(dir, "testnet-key.txt"), deployKey.toString("hex") + "\n");
  await writeFile(join(dir, ".gitignore"), "testnet-key.txt\n");

  // the deploy key's testnet address (offline derivation via CCC's registry)
  const client = new ccc.ClientPublicTestnet();
  const signer = new ccc.SignerCkbPrivateKey(client, hex(deployKey));
  const address = await signer.getRecommendedAddress();

  console.log(`initialized controller project in ${dir}`);
  console.log(`  game id: ${config.gameId}`);
  console.log(`  deploy key address (testnet): ${address}`);
  console.log(`
next steps:
  1. fund the deploy key (~1000 CKB is plenty): https://faucet.nervos.org
  2. ckb-controller deploy --send     (creates YOUR game cell + account; code cells are shared)
  3. ckb-controller status
  4. run the operator + demo (see ckb-controller/demo/README.md)`);
}
