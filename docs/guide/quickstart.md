# Quickstart

There are two tracks. **Track A** runs the entire stack on a local dev chain
with test funds — nothing to fund and nothing at risk. **Track B** deploys a
game to public testnet with a single faucet claim. Both use the same CLI
(`node cli/bin.mjs <command>`; run `npm link` in `cli/` once to invoke it as
`ckb-controller`).

---

## Track A — local, no funds (~10 min)

Everything runs against a throwaway CKB dev chain that the CLI boots for you.

**Prerequisites**

- A checkout of this repository.
- The `ckb` release binaries (a `ckb_v0.2xx` directory). Point
  `networks.devnet.ckbDir` in `controller.config.json` at it, or set the
  `CKBDIR` environment variable.
- The operator binary, built once (this step requires Rust):
  `cargo build -p paymaster-service --bin game-operator`. If it is missing,
  `dev` still boots the chain and prints the exact command to run.

**Steps**

```sh
cd cli && npm install          # once
node cli/bin.mjs dev           # from anywhere in the repo
```

`dev` boots a dev chain under `.controller/devnet-chain`, funds itself from the
well-known dev key, deploys the code cells, game cell, and account, starts the
operator and the demo, and prints a summary:

```
════════════════════ LOCAL STACK UP ════════════════════
 chain    : http://127.0.0.1:8114
 operator : http://127.0.0.1:9944
 PLAY     : http://localhost:5173/game.html?operator=http://127.0.0.1:9944&game=0x…
```

Open the **PLAY** URL to reach the multiplayer scoreboard. Open several tabs to
simulate several players. Each click is a session-signed move committed on-chain
by the operator. `Ctrl-C` tears the stack down; chain state persists across
runs.

There is no faucet, no key to guard, and no cost. To change what a move does,
continue to [your-game.md](./your-game.md).

---

## Track B — public testnet (~15 min)

A real chain with real (test) funds. The only cost is **your** game cell and
account — the shared code cells (lock, ckb-auth, game script) are already
deployed on testnet and pre-seeded into your manifest, so you do not redeploy
~300k CKB of binaries.

### 1. Scaffold (instant)

```sh
node cli/bin.mjs init my-game
cd my-game
```

This writes `controller.config.json` (a fresh game id and fresh keys), a
pre-seeded `.controller/manifest.json`, a new `testnet-key.txt`, and a
`.gitignore` for the key. It prints your **deploy key address**.

### 2. Fund it (a few minutes)

Paste that address into https://faucet.nervos.org and claim. **~1000 CKB is
sufficient** — you pay only for your game cell (~500 CKB) and account
(~500 CKB), because the code cells are shared. Wait for the claim to confirm.

### 3. Deploy (a few minutes)

```sh
node cli/bin.mjs deploy            # dry run — shows exactly what it will create
node cli/bin.mjs deploy --send     # broadcasts, waits for each cell to commit
```

Deploy creates only what is missing. The code cells are skipped (already live
and shared), so on a fresh testnet project this creates your game cell genesis
and your account cell. It waits for each to confirm (a testnet block takes
seconds to a minute).

### 4. Verify

```sh
node cli/bin.mjs status
```

This reports the chain tip, each code cell's liveness, your account (which
should be **1 cell**), your game cell (sequence and player count), and whether
the operator is running.

### 5. Run the operator and play

Start the operator from the **repository checkout**, pointed at *your project's*
config. Without `CONTROLLER_CONFIG` it would read the repository's own config
and the wrong game id:

```sh
cd <repo>
CONTROLLER_CONFIG=../my-game/controller.config.json \
  cargo run -p paymaster-service --bin game-operator      # -> :9944
```

Then serve the demo UI and open it with your game id (found in your config):

```sh
cd <repo>/demo && npm install && npm run dev
# open http://localhost:5173/game.html?operator=http://127.0.0.1:9944&game=0x<your gameId>
```

Full details on running the operator as a service, live Fiber channels, and
testnet-specific issues are in [going-live.md](./going-live.md).

---

**Costs and timing**

| Step | Cost | Time |
|---|---|---|
| `dev` (Track A) | none (dev chain) | seconds to boot |
| `init` | none | instant |
| faucet claim | free test CKB (~1000) | minutes |
| `deploy --send` | game cell (~500) + account (~500) | minutes (waits for commits) |
| running the operator | ~0.001 CKB fee per transition | continuous |

The keys `init` generates are **demo-grade** (plain hex in a file) — acceptable
for testnet, never for mainnet. See the note in
[configuration.md](./configuration.md#a-note-on-keys).
