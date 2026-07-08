# Going live on testnet

This page covers the move from a local dev chain to a public testnet game. It
assumes you have read [quickstart.md](./quickstart.md) Track B; here we cover
running the operator as a service, live Fiber channels, and the testnet-specific
issues that are already solved.

## Recap: get on testnet

```sh
node cli/bin.mjs init my-game        # fresh config + pre-seeded manifest
# fund the printed address at https://faucet.nervos.org  (~1000 CKB)
node cli/bin.mjs deploy --send       # your game cell + account (code cells are shared)
node cli/bin.mjs status              # verify: 1 account cell, live game cell
```

## Running the operator as a service

The operator batches players' session-signed intents and commits them as
game-cell transitions. Run it from the **repository root** with no extra
environment — it reads the config pair next to it. For a scaffolded project,
point it at yours with
`CONTROLLER_CONFIG=/path/to/my-game/controller.config.json`:

```sh
cargo run -p paymaster-service --bin game-operator      # -> :9944
```

It locates the game cell by its type script at startup, then serves `/health`,
`/game` (the board), and `/intent` (post a move). Point clients at it, with the
Vite dev server running in `demo/`:
`http://localhost:5173/game.html?operator=http://127.0.0.1:9944&game=0x<your gameId>`.

**Custom setups** override with environment variables (the same names used
elsewhere):

```sh
CHAIN=http RPC=https://testnet.ckb.dev/rpc \
  KEYFILE=/path/to/testnet-key.txt \
  DEPLOY_FILE=demo/game-deploy.json \
  cargo run -p paymaster-service --bin game-operator
```

**Restart the operator after any out-of-band game-cell change** (such as
`game grow`, or a transition committed by another tool). The operator tracks the
tip from its own flushes; a restart makes it re-locate the current game cell. A
batch the node rejects — for example, a forged intent signature, which is caught
only on-chain — is removed from the queue so it cannot block later players.

## Live Fiber channels

For high-frequency payments, the SDK opens a budget-capped Fiber channel. In the
browser demo the Fiber node runs **in the page** (WASM), and the funding
transaction is signed by the controller **session key** — the player's wallet
key is never handed to the Fiber node. In code:

```ts
const rail = await controller.channel({ mode: "live", peer });
await rail.open(500n);      // ONE session-signed L1 funding tx (external funding)
await rail.waitReady(5n);   // ChannelReady + outbound liquidity (~90 s live)
await rail.pay(5n);         // off-chain, instant, no popup, no L1
await rail.close();         // cooperative settle back to the account
```

Prerequisites for live mode:

- **A WSS-reachable peer.** The browser node needs a native Fiber node (`fnn`)
  on testnet, reachable over WSS. Start one behind a Cloudflare quick tunnel:

  ```sh
  node cli/bin.mjs tunnel
  ```

  It prints a ready-to-paste `?live=1&peer=…&wss=…` demo URL. (Requires bash,
  the `fnn` binary, `cloudflared`, and ~500 testnet CKB on the fnn's own key so
  it can reserve its side of the channel.)

- **An account cell larger than the channel budget.** The lock allows one
  account input per tx, so the account must be a single cell holding more than
  the budget. Top up in place first:
  `node cli/bin.mjs account grow 700 --send`.

- **After settling, drain.** A cooperative close leaves **two** cells (funding
  change plus returned balance). Merge back to one:
  `node cli/bin.mjs account drain --send`.

## Known live-mode issues (already solved)

These are handled in the SDK and the runbooks; know them so an unexpected
console message does not send you debugging a non-issue.

- **The browser's funding-tx broadcast error is expected.** The funding tx
  carries the acceptor fnn's reserve input, so the browser's *local* broadcast
  fails on `Inputs[1].Lock`. But tx hashes are witness-independent and the
  acceptor broadcasts the fully-signed tx, so the channel proceeds. Do not end
  the session on that error.
- **The tunnel hostname changes every run.** Re-copy the URL that `tunnel`
  prints each time; the previous link is dead.
- **Channel state lives in IndexedDB.** The browser holds the open channel, so a
  persistent browser profile survives a restart with the channel intact. An
  orphaned channel can be recovered from the fnn side
  (`shutdown_channel force=true`; funds return after the commitment delay).

Both live demos — the Fiber channel and the multiplayer game aggregator — have
run end-to-end on public CKB testnet. The full walkthroughs are
[demo/README.md](https://github.com/Kagwep/ckb-controller/blob/main/demo/README.md)
and
[demo/GAME-TESTNET.md](https://github.com/Kagwep/ckb-controller/blob/main/demo/GAME-TESTNET.md).
