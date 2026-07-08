# @ckb-controller/cli

Developer CLI for the CKB controller — the "install → configure → game runs"
surface (plan.md, Direction 1B). Everything operates on the config pair from
Direction 1A: `controller.config.json` + `.controller/manifest.json`
(found by upward search from the cwd, `--config <path>`, or
`CONTROLLER_CONFIG`). Env vars (`NETWORK`, `RPC`, `KEYFILE`, `GAME_ID`)
override single values.

```sh
cd cli && npm install         # once
node cli/bin.mjs <command>    # or `npm link` for a global `ckb-controller`
```

## Commands

| command | what it does |
|---|---|
| `init [dir]` | Scaffold a new project: config with a **fresh game id + fresh keys**, a deploy key (+ `.gitignore` for it), and a manifest **pre-seeded with the shared public testnet code cells** — so a new game needs only its own game cell + account (~1k CKB, one faucet claim), not a ~314k CKB redeploy. Prints the funding address + next steps. |
| `status` | One screen of truth: chain tip, each code cell live?, account cells + capacity, game cell seq/players, operator health. |
| `deploy [--send]` | Make the network game-ready, deploying **only what's missing**, in order: code cells (lock / ckb-auth / game script) → game-cell genesis for the config's game id → the controller account cell (`--account-ckb=N`, `--game-ckb=N`). Dry-run by default. Updates the manifest as artifacts land. |
| `account show\|grow <ckb>\|drain [--send]` | Single-cell hygiene (the lock allows ONE account input per tx). `grow` tops up in place (session witness + one sighash input → one bigger cell); `drain` session-signs the smallest cell back to the key — and refuses up front if the amount exceeds the session spend cap (which would fail on-chain). |
| `game show\|grow <ckb> [--send]` | Board + game-cell capacity hygiene. Each player entry occupies its encoded size in capacity (36 CKB on the demo scoreboard); a full cell rejects new players (`InsufficientCellCapacity`). `grow` enlarges the cell in place via an empty-batch no-op transition. Restart the operator afterwards — it tracks the tip from its own flushes. |
| `build [--docker] [--no-wasm]` | Compile the contracts (riscv) + the wasm client from source and sync `wasm/pkg` into `demo/pkg` and `cli/pkg`. The command behind the **game template**: edit `game-rules/src/lib.rs`, `build`, then `deploy --send` (which detects the changed code hash and ships the new script). `--docker` builds the contracts in `docker/build.Dockerfile`'s image instead of the local toolchain. Needs a repo checkout. |
| `dev` | One-shot local stack: boots a CKB dev chain (CKB2023 at epoch 0 for data2/VM2, cellbase maturity 0, instant blocks via `generate_block`) under `.controller/devnet-chain`, funds from the well-known dev key, runs the same deploy core against the local RPC (with the devnet secp dep group discovered from genesis block 0 — CCC's registry is testnet-only), then starts the game-operator (`NETWORK=devnet`) and the demo, and prints the play URL. Ctrl-C tears everything down; chain state persists across runs. Needs `networks.devnet.ckbDir` (or `CKBDIR`) pointing at a ckb v0.2xx release dir. |
| `tunnel` | Expose a testnet fnn over WSS for the browser demo's live mode (wraps `../ckb-controller-cli/run-fnn-wss.sh`; needs bash + fnn + cloudflared). |

## Safety rails

- Deploy/genesis/account transactions select **plain cells only** (no data, no
  type) and assert the final input set — the key's live code cells can never be
  spent by accident (the `fund.mjs` lesson).
- State-changing commands are **dry-run by default**; `--send` broadcasts.
- The wasm pkg is bundled (`cli/pkg`), so standalone projects don't need the
  Rust toolchain; a repo checkout's own `wasm/pkg` / `demo/pkg` takes precedence.

## Known deployments

`lib/known-deployments.mjs` ships the public testnet out-points of the lock,
ckb-auth, secp dep group, and game script. `deploy` verifies they're still live
before relying on them. Prefer your own copies? Delete the entries from your
manifest and re-run `deploy --send` with the binaries built.
