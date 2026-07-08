# controller-demo

Browser demos of the CKB controller. Two rails, matching the design's
value/state split:

- **Channel demo** (`index.html`) — *value*: **one approval → session +
  budget-capped Fiber channel → buy troops (off-chain, gasless, no popups) →
  settle on L1.** In live mode the Fiber node runs **in the page** (WASM).
- **Game demo** (`game.html`) — *state*: N tabs = N players; each move is a
  session-signed **intent** posted to an operator that batches them into
  on-chain game-cell transitions, verified by the type script.

The pitch the raw-Fiber game demo can't make: *the Fiber node can live in your
browser, yet a compromised game loses at most the channel budget — never your
wallet.*

Three tiers, by what they cost:

- **Mock** — everything in-browser on the wasm in-memory rail. No node, no
  chain, **no funds**.
- **Local** — a local dev chain + local fnn nodes, funded by the genesis dev
  key (nothing to fund). Driven by the runbooks in `../../ckb-controller-cli`
  (`run.sh`, `run-channel.sh`), not by these pages.
- **Live** — **public testnet with actual funds**: a faucet-funded key pays
  for the code cells, the account cell, and the fnn's reserve
  (https://faucet.nervos.org). This is what `?live=1` and the operator mode
  below mean by "live".

Both live demos are **proven on public CKB testnet** (2026-07-08): a full
open → pay×5 → settle from the in-browser Fiber node, and live game
transitions committed through the operator with forged intents rejected
on-chain.

**Configuration:** everything below reads `../controller.config.json` +
`../.controller/manifest.json` (network, keys, session policy, deployed code
cells). Env vars (`RPC`, `KEYFILE`, `GAME_ID`, `NETWORK`) override single
values. No script hardcodes a deploy point anymore.

## Run (mock mode — works now, no node, no funds)

```sh
# 1. build the wasm package (from repo root), once:
./scripts/build-wasm.sh

# 2. run the demo:
cd demo
npm install
npm run dev          # -> http://localhost:5173
```

`npm run dev` auto-copies `../wasm/pkg` into `demo/pkg` first (see
`scripts/copy-pkg.mjs`).

Click **Approve session & open channel** → **Buy troops** (each click is an
off-chain channel payment, tracked against the budget) → **Cash out & settle**.
Everything runs in-browser on the wasm `ChannelSession` (in-memory rail): real
controller address, real session signatures, real funding/settle tx shapes — just
not broadcast.

Point it at a deployed testnet lock to show the real address:
`http://localhost:5173/?lock=0x<your-lock-code-hash>`.

## Live channel demo (in-browser Fiber WASM node, public testnet)

`@ckb-controller/sdk`'s `LiveRail` (this demo is the SDK's reference consumer;
`src/rail.ts` is the thin adapter) runs the production path: an actual Fiber
node in the browser
(`@nervosnetwork/fiber-js`, matching the native fnn build) funded via
`openChannelWithExternalFunding`, with the funding tx signed by the controller
**session key** — the user's wallet key is never handed to the Fiber node.
COOP/COEP headers are already set in `vite.config.ts`.

### Prerequisites (live = testnet = actual funds)

- The controller lock + ckb-auth deployed on testnet (resolved from
  `.controller/manifest.json` via the SDK) and a funded **controller account
  cell** holding more
  than the channel budget — see below. Deploying from scratch costs ~246k CKB
  in code cells (one faucet claim, reclaimable) — see
  `../../ckb-controller-cli/run-testnet.sh`.
- A peer for the browser node: a native `fnn` on testnet, reachable over
  **WSS**. `../../ckb-controller-cli/run-fnn-wss.sh` starts one behind a
  Cloudflare quick tunnel and prints everything you need (see that script's
  header for its own prereqs: fnn binary, cloudflared, and ~500 testnet CKB
  on the fnn's ckb key — it needs a real reserve to accept the channel).

### Steps

```sh
# 1. account hygiene — the lock allows ONE account input per tx, so the account
#    must be a single live cell with more capacity than the channel budget:
node check-account.mjs               # lists the account's live cells
node grow-account.mjs 700 send       # top up IN PLACE (session-signed; single cell)
node drain-account.mjs               # merge back to one cell (run after a settle —
                                     # settle leaves 2 cells: funding change + return)

# 2. start the WSS peer (from ../../ckb-controller-cli). It prints the demo URL:
./run-fnn-wss.sh

# 3. serve the demo and open the printed URL:
npm run dev
#   http://localhost:5173/?live=1&peer=<fnn pubkey>&wss=/dns4/<host>/tcp/443/wss/p2p/<peerid>
```

Then click through: approve (funds the channel with a session-signed L1 tx) →
buy (off-chain, instant) → settle (cooperative close back to the account).

Headless click-through (puppeteer-core + system Edge, persistent profile):

```sh
DEMO_URL=http://localhost:5173 node run-live.mjs <peerPubkey> <wssMultiaddr> [budgetCkb] [buys]
```

### Live-mode gotchas (all hit and solved; don't rediscover them)

- **The browser's own funding-tx broadcast error is a red herring.** The funding
  tx carries the acceptor fnn's reserve input, so the browser's local
  `send_transaction` fails `Inputs[1].Lock` — but tx hashes are
  witness-independent and the acceptor broadcasts the fully-signed tx. Don't
  kill the session on that console error; the channel proceeds.
- **The quick-tunnel hostname changes every run** — re-copy the URL that
  `run-fnn-wss.sh` prints; yesterday's link is dead.
- **Channel state lives in IndexedDB** — `run-live.mjs` uses a persistent Edge
  profile (`.edge-profile/`) so an open channel survives a restart. An orphaned
  channel can be recovered from the fnn side with
  `shutdown_channel force=true` (funds return after the commitment delay).
- **Git Bash mangles multiaddrs** (`/dns4/…` → `C:/Program Files/Git/dns4/…`)
  when passed as command-line args — invoke node drivers with
  `MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*"`. (Pasting the URL into a browser
  is unaffected.)
- The default channel budget is **500 CKB** — grow the account first if it
  holds less (`check-account.mjs` / `grow-account.mjs`).

## Live game demo (multiplayer aggregator, public testnet)

Full bring-up (deploy the game type script, genesis the game cell) is in
[`GAME-TESTNET.md`](./GAME-TESTNET.md). With those done once, a live session is:

```sh
# operator (from repo root) — finds the live game cell by type script, batches
# intents, finalizes + broadcasts transitions:
CHAIN=http RPC=https://testnet.ckb.dev/rpc \
  KEYFILE=<funded testnet privkey file> DEPLOY_FILE=demo/game-deploy.json \
  cargo run -p paymaster-service --bin game-operator      # -> :9944

# then open (several tabs = several players):
#   http://localhost:5173/game.html?operator=http://127.0.0.1:9944

# or drive it from the CLI (same client path as the page):
node post-intent.mjs                                  # fresh player, +5 on the live board
node post-intent.mjs http://127.0.0.1:9944 5 forge    # forged sig -> rejected ON-CHAIN
```

Every move is a session-signed intent; the on-chain type script re-derives the
transition and checks every intent signature via ckb-auth — the operator
provides liveness, never safety.

## What's real vs mocked

| Piece | Mock mode | Live mode (proven on testnet) |
|---|---|---|
| Controller address / session args / policy | ✅ wasm | ✅ wasm |
| Session signatures (recoverable secp256k1) | ✅ @noble | ✅ @noble |
| Funding / settle tx shapes | ✅ wasm builds them | Fiber builds, session signs |
| Off-chain payments | tracked in-memory (MockRail) | Fiber TLCs over WSS |
| Game moves | — | session-signed intents, type-script-verified |
| Broadcast / on-chain | no | yes (public testnet) |
