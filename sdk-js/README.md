# @ckb-controller/sdk

The runtime SDK for the CKB controller — what a game dev codes against
(plan.md Direction 1C). Both rails behind one object:

```ts
import init, * as wasm from "controller-wasm/pkg";        // the wasm pkg you ship
import { Controller } from "@ckb-controller/sdk";

await init();
const controller = Controller.load({ config, manifest, wasm });

// STATE rail — session-signed moves, verified on-chain by the game type script
const game = controller.game();          // operator URL + game id from config
const player = game.player();            // fresh session key, nonce managed
await player.move(5n);                   // sign → post → operator commits on-chain
await game.board();                      // the shared scoreboard

// VALUE rail — a budget-capped payment channel
const rail = await controller.channel({ mode: "live", peer });  // or { mode: "mock", budgetCkb }
await rail.open(500n);                   // ONE session-signed L1 funding tx (external funding)
await rail.waitReady(5n);                // ChannelReady + outbound liquidity (~90 s live)
await rail.pay(5n);                      // off-chain, instant, no popup, no L1
await rail.close();                      // cooperative settle back to the account
```

What it hides: witness assembly, the cell_deps-cleared signing message, policy
proof regions, account-lock derivation, Fiber's external-funding contract
("witness-only, never touch inputs/outputs"), the funding-broadcast red
herring, peer-handshake and channel-readiness polling, per-player nonces.
What it exposes: the session limits — the budget IS the spend cap the lock
enforces; a compromised game loses at most the channel budget, never the
wallet.

## Design

- **Pure logic, injected dependencies.** `Controller.load({ config, manifest,
  wasm })` takes the parsed config pair (`controller.config.json` +
  `.controller/manifest.json`) and the **initialised** controller-wasm module.
  The SDK does no file or wasm loading, so it runs identically under Vite, any
  bundler, or plain Node.
- **fiber-js is an optional peer dep**, imported lazily inside
  `LiveRail.create` — mock-only consumers never load the ~14 MB Fiber bundle.
- Byte layouts implemented here are specified in
  [`docs/internals/wire-formats.md`](../docs/internals/wire-formats.md) — a
  change there must change this package in the same PR.

## Build / consume

```sh
npm install && npm run build      # tsc -> dist/ (ESM + .d.ts)
```

The demo (`../demo`) consumes it as `file:../sdk-js` and is the reference
integration: `demo/src/game.ts` (state rail UI) and `demo/src/rail.ts`
(channel rail adapter) are each under a hundred lines. Bundler note: with a
file-linked install, dedupe the shared deps
(`resolve.dedupe: ["@ckb-ccc/core", "@noble/curves", "@noble/hashes"]` in
Vite) so only one copy is bundled.

## Modules

| module | contents |
|---|---|
| `index` | `Controller` facade (`load`, `game()`, `channel()`, `explorerTx`) |
| `account` | `deriveAccount` — registered-model lock from config policy, byte-identical to the deployed cell |
| `session` | `Session` — tx-message signing, witness assembly, witness-only `signFundingTx`, `fundingTxHash` |
| `game` | `GameClient` / `GamePlayer` — intents, nonce management (rollback on reject), board |
| `mock` | `MockRail` — the wasm in-memory channel loop (no node, no funds) |
| `live` | `LiveRail` — the in-browser Fiber WASM node path, with every live-mode gotcha baked in |
| `keys` | blake160 pubkey hashes, 65-byte recoverable signatures |
