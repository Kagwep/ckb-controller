#!/usr/bin/env node
// ckb-controller — developer CLI for the CKB controller.
//
//   init      scaffold controller.config.json + .controller/ in a directory
//   status    one-screen health check: chain, code cells, account, game, operator
//   deploy    deploy whatever's missing on the configured network, update manifest
//   account   show | grow <ckb> | drain   (single-cell hygiene, session-signed)
//   game      show | grow <ckb>           (board + game-cell capacity hygiene)
//   build     compile contracts (riscv) + wasm from source, sync pkgs
//             (--docker to build contracts in the toolchain image)
//   dev       one-shot local stack: dev chain + deploys + operator + demo
//   tunnel    expose a testnet fnn over WSS for the browser demo's live mode
//
// Global flags: --config <path> (else CONTROLLER_CONFIG env, else upward search),
// --send on state-changing commands (default is dry-run).

const [, , cmd, ...rest] = process.argv;

function takeFlag(args, name) {
  const i = args.indexOf(name);
  if (i === -1) return { args, value: undefined };
  const value = args[i + 1];
  return { args: [...args.slice(0, i), ...args.slice(i + 2)], value };
}

const { args, value: configPath } = takeFlag(rest, "--config");

const commands = {
  init: () => import("./commands/init.mjs"),
  status: () => import("./commands/status.mjs"),
  deploy: () => import("./commands/deploy.mjs"),
  account: () => import("./commands/account.mjs"),
  game: () => import("./commands/game.mjs"),
  build: () => import("./commands/build.mjs"),
  dev: () => import("./commands/dev.mjs"),
  tunnel: () => import("./commands/tunnel.mjs"),
};

if (!cmd || !(cmd in commands)) {
  console.error(`usage: ckb-controller <${Object.keys(commands).join("|")}> [args] [--config path] [--send]`);
  process.exit(2);
}

try {
  const mod = await commands[cmd]();
  await mod.run(args, { configPath });
} catch (e) {
  console.error(`error: ${e.message ?? e}`);
  process.exit(1);
}
