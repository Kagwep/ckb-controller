# Multiplayer aggregator — testnet bring-up runbook

Proves the aggregator + game-cell loop **on CKB testnet**: deploy the game type
script, genesis an empty game cell, then commit a real **session-signed transition**
that the on-chain type script verifies (every intent signature checked via ckb-auth,
the state transition re-derived). This is the multiplayer analog of the channel
demo's live "1 fund + N pays + 1 settle".

## What runs where

- **State** (this runbook) → aggregator + game cell on L1.
- **Value** (the other demo) → Fiber channel.

The three scripts are CCC `.mjs` (like `topup`/`fund`/`drain-account.mjs`) and each
defaults to a **dry run**; add `send` to broadcast. Results are written to
`game-deploy.json` and `game-cell.json`, which later steps read.

## Prerequisites

- Build the type script: from repo root `./build.sh` (produces
  `build/release/controller-game-cell`, ~68 KB).
- Build + sync the wasm pkg: `./scripts/build-wasm.sh` then, in `demo/`,
  `node scripts/copy-pkg.mjs` (advance.mjs computes the next state with it).
- A **funded** testnet key at `D:/projects/ckb-controller-cli/testnet-key.txt`
  (override with `KEYFILE=`). **Deploying the type script costs ~68k CKB**
  (1 CKB/byte); claim from faucet.nervos.org. The genesis cell is only ~500 CKB.
- Reuses the ckb-auth code cell already deployed for the controller lock
  (`AUTH_DEP` in `game-config.mjs`); no need to redeploy the 151 KB auth binary.

## Steps

```bash
cd demo

# 1) Deploy the game type script code cell (~68k CKB). Writes game-deploy.json.
node game-deploy.mjs           # dry run: prints code_hash, capacity, inputs
node game-deploy.mjs send

# 2) Genesis: create the empty game cell (type=game script, args=GAME_ID, empty
#    state). The node runs the type script (genesis path) — success proves the
#    deploy. Writes game-cell.json.
node game-genesis.mjs send

# 3) Advance: commit a session-signed transition. Two demo players each score +5;
#    intents are signed with the SAME message the browser signs, the next state is
#    computed with wasm game_apply, and the node verifies every intent sig via
#    ckb-auth + the exact transition. Re-run to accumulate (nonces auto-increment).
node game-advance.mjs          # dry run: prints the intents + next seq
node game-advance.mjs send
```

Each `advance … send` commits one transition tx (the game cell self-funds the
0.001 CKB fee by shrinking a hair, so the only input is the game cell — no fee-cell
selection, no risk to the code cells the key also holds). Inspect the game cell's
data on the explorer, or re-run `game-advance.mjs` (dry) to see the decoded board.

## Config knobs (env)

- `RPC` (default `https://testnet.ckb.dev/rpc`), `KEYFILE`, `GAME_ID`
  (default `0x00…00`, **must equal `demo/src/game.ts`'s default** so browser intents
  match), `GAME_CELL_CKB` (genesis size, default 500), `POINTS` (per move, default 5).

## Scope / boundary

- This proves the on-chain aggregator with the CCC driver acting as the operator
  (it batches the demo players' intents and commits the transition). The **safety**
  claim is what gets proven live: forged/replayed intents are rejected by the type
  script, not by the driver.
- The `game-operator` bin also runs LIVE against testnet (`CHAIN=http`, wired +
  proven 2026-07-08): it locates the game cell by type script at startup, and each
  `/intent` is batched, finalized with the operator key's sighash-all signature
  (in Rust, `paymaster_service::sighash`), and broadcast — the same self-funded
  single-input shape as `game-advance.mjs`:

  ```bash
  CHAIN=http RPC=https://testnet.ckb.dev/rpc \
    KEYFILE=D:/projects/ckb-controller-cli/testnet-key.txt \
    DEPLOY_FILE=demo/game-deploy.json \
    cargo run -p paymaster-service --bin game-operator
  # then open game.html?operator=http://127.0.0.1:9944, or:
  node post-intent.mjs                       # fresh player, +5 on the live board
  node post-intent.mjs http://127.0.0.1:9944 5 forge   # on-chain sig rejection demo
  ```

  A node-rejected batch (e.g. forged sig — only checked on-chain) is shed from the
  queue so it can't jam later players.
