# Phase 2 live run — hub + trampoline (runbook)

> **Status: supervised-run procedure.** The code side of Phase 2 (see
> [multi-user-plan.md](./multi-user-plan.md)) is built and statically verified;
> this page is the operator checklist for the live testnet run: two browsers,
> one hub, A → hub → B. Everything here is testnet, low-value, and reversible —
> but the funding scripts touch the sighash key that also holds the live code
> cells, so follow the safety notes exactly.

## Topology

```
Browser A (per-user keys, ?multi=1)     hub fnn (native, testnet)     Browser B
  controller-locked channel ──────────►   ◄────────── controller-locked channel
  sendPayment({ invoice,                trampoline: hub pathfinds
    trampoline_hops:[hubPubkey],        the hub→B leg out of ITS OWN
    max_fee_amount })                   balance in the hub↔B channel
```

Both channel opens are the proven single-user LiveRail path (session-signed
external funding). The only new mechanics are: per-user accounts (Phase 1), the
invoice (`newInvoice`/`waitInvoicePaid` on B), and the trampoline-routed
`payInvoice` on A.

## 1. Hub bring-up

The hub is the same fnn the single-user demo peers with —
`D:/projects/ckb-controller-cli/run-fnn-wss.sh` (fnn at
`D:/projects/fiber/target/release/fnn.exe`, node dir
`/d/projects/ckb-bin/work/fnn-testnet`, cloudflared quick tunnel). Run it from
Git Bash **with the liquidity knob** (section 2):

```bash
cd /d/projects/ckb-controller-cli
FIBER_SECRET_KEY_PASSWORD=controller-demo \
FIBER_AUTO_ACCEPT_CHANNEL_CKB_FUNDING_AMOUNT=19900000000 \
./run-fnn-wss.sh
```

It prints the ready `?live=1&peer=<pubkey>&wss=/dns4/<host>/tcp/443/wss/p2p/<peerId>`
pair. Notes:

- The wss dial multiaddr **must** include `/p2p/<peerId>` — the browser node
  cannot extract the peer id otherwise and silently refuses to connect.
- The quick-tunnel host changes every run; re-copy the URL each time.
- **MSYS mangles multiaddrs** (`/dns4/…` looks like a path). Any Git Bash
  command that passes a multiaddr as an argument needs
  `MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*"`. The script itself is safe (it
  only prints); the hazard is ad-hoc `curl`/`node` invocations you type.
- **Gossip ban must stay disabled on BOTH sides** — `gossip_policy: ban:
  threshold: 4294967295`. Already applied to the hub config
  (`/d/projects/ckb-bin/work/fnn-testnet/config.yml`) and the browser config
  (`demo/public/fiber-config/testnet.yml`); verify both before the run. Without
  it the fnn bans the syncing browser node ~1 min after channel-ready.

## 2. Hub liquidity — how B gets receivable capacity

This is the part that does NOT come for free. When B opens its channel, B funds
its own side — that gives the hub **inbound** capacity from B, but the hub's
side of that channel is only what the hub contributed as acceptor. The A→hub→B
payment spends the **hub's own balance in the hub↔B channel** for the final leg
(a TLC reimburses it on the A↔hub channel), so the hub needs spendable balance
toward B *before* B can receive anything.

The acceptor contribution is controlled by fnn's auto-accept config
(`crates/fiber-lib/src/fiber/config.rs`):

| knob (env form) | default | meaning |
|---|---|---|
| `FIBER_AUTO_ACCEPT_CHANNEL_CKB_FUNDING_AMOUNT` | 99 CKB | what the acceptor contributes to an auto-accepted channel; `0` disables auto-accept |
| `FIBER_OPEN_CHANNEL_AUTO_ACCEPT_MIN_CKB_FUNDING_AMOUNT` | 100 CKB | minimum opener funding to qualify for auto-accept |

**The default 99 CKB is a trap: it is exactly the channel reserve**
(98 CKB commitment-lock occupied capacity + 1 CKB shutdown fee), i.e. **zero
spendable outbound liquidity** toward the opener. With defaults, the hub→B leg
fails for any amount.

Fix (recommended): raise the acceptor contribution via env when starting the
hub — fnn's clap config reads it directly, no config-file edit needed:

```bash
# 199 CKB per accepted channel = 99 reserve + 100 CKB spendable toward the opener
FIBER_AUTO_ACCEPT_CHANNEL_CKB_FUNDING_AMOUNT=19900000000 ./run-fnn-wss.sh
```

Size it to the expected receive volume: spendable-toward-B = contribution −
99 CKB, and B can receive at most that much before the hub's side is drained
(payments in the other direction refill it). The hub's ckb key must hold
`contribution × channels + fees` on top of its existing reserve — top it up at
https://faucet.nervos.org (or `faucet-api.nervos.org` HTTP claim) if the run
plans two channels at 199 CKB each.

Alternatives, for reference: (a) the hub can *manually* open a channel toward B
(`open_channel` RPC over the already-connected peer) with any funding it likes;
(b) B could keysend the hub first to build hub-side balance — useless for a
pure receiver. The env knob is the operationally simple one and applies
symmetrically to both players' channels, so payments work in both directions.

Opener-side constraint (learned the hard way): the auto-accept minimum is
checked against the funding amount **net of Fiber's 158 CKB channel occupancy**
— a 200 CKB budget counts as only 42 CKB and silently stalls "pending manual
acceptance" (the hub log is the only place this shows). So either budget ≥
min + 158 (258 CKB with the default 100 min), or lower the hub's minimum:
`FIBER_OPEN_CHANNEL_AUTO_ACCEPT_MIN_CKB_FUNDING_AMOUNT=4000000000` (40 CKB net)
alongside the contribution knob.

## 3. Fund the two per-user accounts

Fresh per-user accounts hold ZERO CKB. Each browser needs one account cell
before it can open a channel:

1. Serve the demo (`cd demo && npm run dev`), then mint/read each profile's
   address headlessly: `node get-user-addr.mjs demo/.edge-profile-userA` (and
   `…userB`) — it loads `?multi=1` in that profile and prints the full `ckt1…`
   address. (Manual alternative: open the URL in the profile; the footer shows
   `this browser: ckt1…` with the full address in the tooltip/log.)
2. Fund each address from the sighash deploy key:

```bash
cd demo
node fund-user.mjs ckt1...userA 700        # dry run — inspect inputs/outputs
node fund-user.mjs ckt1...userA 700 send   # broadcast
node fund-user.mjs ckt1...userB 700 send
```

Safety properties of `fund-user.mjs` (do not bypass them):

- The sighash key also holds the **95k/151k/68k code cells that are live
  cell-deps** — the script pins one plain cell (dataLen=0, no type) and
  hard-asserts it is the only input. If it aborts, do NOT hand-roll a transfer.
- The account lock rejects >1 account input (`MultipleInputs`), so an account
  must stay a **single live cell** — the script refuses to fund an address that
  already has one. Growing an existing per-user cell needs that user's session
  key (which lives in the browser's localStorage); the grow-account.mjs pattern
  applies but is not scripted for per-user keys yet.
- 700 CKB each: 500 max budget + ~170 change-cell reserve + margin. The
  spend cap is 1000 CKB; keep budgets ≤ ~800.

## 4. The two-browser run

```bash
cd demo
npm run dev                       # note the port vite prints (5173 by default)
DEMO_URL=http://localhost:5173 \
node run-live-multi.mjs <hubPubkey> "<wssMultiaddr>" 300 20
```

(Quote the multiaddr; in Git Bash also prefix `MSYS_NO_PATHCONV=1
MSYS2_ARG_CONV_EXCL="*"`.)

Stages (each logged, fail-fast): launch A+B with separate persistent profiles
(`.edge-profile-userA/B` — separate localStorage = separate identities; the
driver asserts the two accounts differ) → both connect + open channels
(~2–3 min each, parallel) → B requests a 20 CKB invoice → A pastes + pays via
`trampoline_hops=[hub]` → assert B's page shows 20 CKB received → both settle.

The same flow works by hand: open the URL in two normal browser profiles and
use the "P2P pay (routed via the hub)" panel.

## 5. Expected costs (per run, testnet CKB)

| what | who pays | ~how much |
|---|---|---|
| account funding | sighash key → each user | 700 × 2 |
| channel funding | each user's account (returns at settle, minus spend) | budget (300 default) |
| acceptor contribution | hub ckb key (returns at settle) | 199 × 2 |
| funding + settle tx fees | funder / closer | < 1 each |
| trampoline routing fee | payer A | bounded by `maxFeeCkb` (UI caps at 10) |

Fee estimation with gossip off is rough — the UI's generous 10 CKB
`max_fee_amount` cap is deliberate; don't trim it for the first run.

## 6. Known red herrings & limits

- **Funding-broadcast `Inputs[1].Lock` -11 error in the browser console is NOT
  fatal**: the acceptor fnn broadcasts the fully-signed same-hash tx. The SDK
  ignores it; you should too.
- Channel is routable only ~90 s **after** the funding tx commits
  (`waitReady` handles this; "max outbound liquidity 0" right after funding is
  the same non-bug).
- Settle during in-flight TLCs fails with "pending outbound tlcs" — the UI
  retries; a second click also works.
- Trampoline is pre-1.0: **5-hop limit** (ours is 2), and the payer must have a
  *direct* channel to the first trampoline hop (it does — the hub IS the
  channel peer).
- If a payment fails with a routing error, check the hub's spendable balance
  toward B first (section 2) — it is the most likely cause by far.

## 7. Phase 3 match run — state + value in one loop

Phase 3 adds a **non-custodial game server** (the operator) that runs shared game
state AND relays invoices, without ever touching funds. The value rail is exactly
Phase 2's (channels to the hub, trampoline pay); what's new is the **state rail**
(session-signed intents → shared game cell) and an **invoice relay** the operator
holds so a payer never copy-pastes. The demo page is `game.html` (not
`index.html`); the driver is `run-live-match.mjs`.

This section extends — it does not replace — sections 1–5. The hub bring-up
(§1), acceptor liquidity (§2), and per-user funding (§3) are prerequisites here
too.

### 7.1 Operator bring-up

The operator is the `game-operator` bin (tiny_http; permissive CORS). Two modes:

```bash
# mock: in-memory game cell, no chain, no funds — the shared STATE is genuinely
# shared across browsers (they all hit this one process), but nothing is on-chain.
CHAIN=mock cargo run -p paymaster-service --bin game-operator      # :9944

# http: the live path — locates the deployed game cell by type script and
# finalizes each transition with the operator key's sighash signature.
# Needs the game genesis committed (game-genesis.mjs) and KEYFILE set.
CHAIN=http KEYFILE=/d/projects/ckb-controller-cli/testnet-key.txt \
  cargo run -p paymaster-service --bin game-operator
```

**KEYFILE must be set explicitly** (for the operator AND the demo `.mjs`
scripts): the config's relative `./testnet-key.txt` resolves against the
process cwd, and the key actually lives in the sibling
`ckb-controller-cli` repo. Same for `RESULTS_FILE` — set it to an absolute
path or the match log lands wherever the operator happened to start
(the 2026-07-10 attempts left NO findable log for exactly this reason).

Endpoints (all JSON): `GET /health`, `GET /game`, `POST /intent`, `POST /flush`
(state); `POST /invoice`, `GET /invoice?for=<hash>`, `POST /invoice/paid`
(invoice relay); `GET /results?n=<N>` (match log). The match log is a JSONL file
(`RESULTS_FILE`, default `game-results.jsonl` in the cwd) — score /
invoice-published / invoice-paid events, append-only. The **operator never sees a
key or preimage** — invoice STRINGS only; value moves over Fiber TLCs.

Smoke it without a browser (mirrors the aggregator smoke): `curl` a publish →
`GET /invoice?for=<payer>` → `POST /invoice/paid {id}`, and confirm `GET /results`
shows both events plus any `POST /intent` score.

### 7.2 The drain caveat (must do before a live match)

After the Phase 2 run, each per-user account holds **2 live cells** (the funding
change cell + the settle return). The account lock forbids a multi-input tx, so a
fresh channel open fails **`MultipleInputs`** — the pre-flight in `rail.ts`
(`liveAccountInfo`) blocks it up front with a "drain" message rather than a
post-broadcast failure.

Before the live match, restore each account to a **single cell**, two options:

- **Fresh profiles** (simplest): delete `.edge-profile-userA/B` (or point
  `PROFILE_DIR_A/B` at new dirs). New profiles mint new zero-cell identities —
  then re-run §3 (`get-user-addr.mjs` → `fund-user.mjs`) to fund them. Note this
  *orphans* the old channels' settle cells under the old identities.
- **Drain to one cell**: consolidate the two cells back into one under the same
  account lock. This needs that user's **session key**, which lives only in the
  browser profile's `localStorage` (`ckb-controller.userKeys.v1`) — a Node helper
  would have to export it via a page-eval the way `get-user-addr.mjs` reads the
  address. That script is **not written** (the session privkey never leaves the
  browser today); fresh profiles are the supported path for now.

Mock mode has no accounts, so this caveat is live-only.

### 7.3 Driver invocation

```bash
cd demo
npm run dev                                   # note the port (5173 default)
CHAIN=mock cargo run -p paymaster-service --bin game-operator   # separate shell

DEMO_URL=http://localhost:5173 OPERATOR_URL=http://127.0.0.1:9944 \
node run-live-match.mjs <hubPubkey> "<wssMultiaddr>" 300
```

(Quote the multiaddr; in Git Bash also prefix `MSYS_NO_PATHCONV=1
MSYS2_ARG_CONV_EXCL="*"`.) Stages (each logged, fail-fast): launch A+B (separate
profiles → separate identities; asserted distinct) → both open controller-funded
channels to the hub → both score (assert the shared board shows both players) →
scoring auto-publishes a **bounty** invoice via the relay → A pays the next
bounty (B's) via `trampoline_hops=[hub]` + the relay → assert the operator's
`/results` shows an `invoice_paid` event → both settle → dump `/results`.

### 7.4 What mock mode simulates — and what it doesn't

The bounty rule: **scoring publishes a 5 CKB invoice** the scorer wants paid; a
**"Pay next bounty"** button fetches the next relayed invoice and pays it.

| | shared across tabs? | notes |
|---|---|---|
| **State** (board, intents, results) | **yes** — via the operator | one operator process; all tabs see the same board + match log even in `CHAIN=mock` |
| **Invoice relay** (the strings) | **yes** — via the operator | publish/fetch/mark-paid all work cross-tab in mock |
| **Value settlement** (`MockRail`) | **no** — per page | `MockRail`'s invoice registry is a module-level map; **two tabs don't share it**, so a bounty published in tab A can't actually be *paid* by tab B's `MockRail` (it throws "unknown invoice"). |

So in **mock** mode the full loop (publish → pay → settle) only closes **within a
single tab** (that tab's rail issued the invoice, so its own registry has it — the
demo omits the payer-hash filter in mock precisely so a tab can pay a bounty it
issued). Cross-tab, mock demonstrates **shared state + the relay round-trip**, not
value receipt. **Live** mode is where value genuinely routes A → hub → B (each
browser's Fiber node issues/pays real invoices); that is what `run-live-match.mjs`
exercises and the supervised run proves. The state rail and the relay are the same
in both modes.

> **Proven live 2026-07-10**: full match PASS with fresh profiles + budget 300 —
> channels `0x7d809ff0…`/`0x6f5cb02a…`, game cell seq 18→20, 5 CKB bounty paid
> A→hub→B, settles at block 21706549 verified **+5 at B / −6 at A** on-chain.
> Two display gotchas from that run: B's settle UI can show a stale `local`
> figure (trust the settle tx), and a transient `HoldTlcTimeout` on the payee's
> console does not mean the payment failed.

## 8. Real multi-device test — one command

The headless driver (§7) proves the loop on one machine. The multi-device test
is the same match with **each player on a real device** (phone, another laptop —
any network). Everything a device needs is HTTPS: the page must be a secure
context (`crossOriginIsolated` for the WASM node's SharedArrayBuffer, and
`https:` pages can't call `http:` operators — mixed content), so the demo AND
the operator each get a Cloudflare quick tunnel next to the hub's existing one.
`vite.config.ts` already sends the COOP/COEP headers and allows
`.trycloudflare.com` hosts.

### 8.0 Fresh setup device — prerequisites (any OS)

Both scripts are plain POSIX bash — they run identically on **macOS, Linux, and
Windows (Git Bash)** — and carry **no machine-specific hardcoding**: tools are
found on PATH, the ckb-controller repo is resolved as a sibling checkout of
ckb-controller-cli, and the hub key's address/lock_arg are read from
`$FNN_DIR/ckb/key-info.json` (written by `run-fnn-wss.sh` the moment it
generates the key — fnn encrypts the key file on first start, so it can't be
derived later). A clean setup machine needs:

- the two repos cloned side by side (`ckb-controller`, `ckb-controller-cli`);
- on PATH: `node`/`npm` (+ `npm install` in `demo/`), `cargo`, `python3`,
  `cloudflared`, `ckb-cli`, and `fnn` (build the
  [fiber](https://github.com/nervosnetwork/fiber) repo release profile; or set
  `FIBERDIR=/path/to/fiber` / `FNN=/path/to/fnn` instead of PATH);
- optionally `WORK=/path/to/writable/workdir` (node store + logs, shared by
  both scripts) — defaults to `~/.ckb-controller/work` (a pre-existing legacy
  Windows dir is honoured);
- a funded testnet sighash key at `ckb-controller-cli/testnet-key.txt` (or
  `KEYFILE` env) — this key deploys nothing new (the code cells are already
  live on testnet, pinned by `.controller/manifest.json`), it only funds
  per-device accounts and operator fees;
- first hub run: `./run-fnn-wss.sh` generates the fnn key, prints its address,
  and exits — fund it (~500 CKB or let the §8.1 script auto-claim) and re-run.

Windows-only notes: run everything from Git Bash, and remember the §1 MSYS
caveat if you type multiaddrs into ad-hoc commands (the scripts themselves are
hardened against it).

### 8.1 Bring-up (the PC)

```bash
cd /path/to/ckb-controller-cli
./run-multidevice.sh
```

One command, from Git Bash. It starts the hub (`run-fnn-wss.sh`, liquidity knob
preset), **auto-tops-up the hub key from the faucet** if its balance is below
500 CKB, starts vite + the `CHAIN=http` operator (KEYFILE/RESULTS_FILE preset),
opens the two extra tunnels, and prints ONE URL:

```
https://<demo-host>/game.html?live=1&multi=1&peer=<hubPubkey>&wss=<enc>&operator=<enc>
```

Ctrl-C tears everything down. Logs land in `/d/projects/ckb-bin/work/`.

### 8.2 Per device

1. **Open the URL** in the device's browser. The page shows
   `this device: ckt1… [copy address]` — identities are minted per-origin on
   first load (`userKeys.ts` localStorage), so each device is automatically a
   distinct player.
2. **Fund it** from the setup machine (paste the copied address into chat/notes
   to move it; the §8.1 READY banner prints this command with the right paths):

   ```bash
   cd ckb-controller/demo
   KEYFILE=../../ckb-controller-cli/testnet-key.txt \
     node fund-user.mjs <address> 700 send
   ```

3. **Open channel** (default budget 300; ~2–3 min; the `Inputs[1].Lock -11`
   console error is the §6 red herring). Wait for "Channel ready ✓".
4. **Score** on both devices — the shared leaderboard must show both players,
   and each score publishes a 5 CKB bounty to the relay.
5. **Pay next bounty** on ONE device — it pays the *other* player's bounty via
   the hub (trampoline). The match log card shows `💰 bounty paid`.
6. **Settle** on both. Verify like §7: `GET <operator>/results` has the
   `invoice_paid` event, and the settle returns are ±5 CKB on-chain.

### 8.3 Multi-device caveats

- **Every run needs fresh funding**: quick-tunnel hostnames change per run, and
  identities are per-origin — a new run = a new origin = new zero-CKB
  identities (700 CKB each). This also conveniently sidesteps the §7.2 drain
  caveat. A stable origin (Cloudflare named tunnel) would make identities—and
  the 2-cells-after-settle problem—persistent; not built yet.
- The budget must leave the account change cell ≥ ~178 CKB (the account cell's
  own occupied capacity): with 700 funding, budgets up to ~520 work; a 400-CKB
  account cannot fund a 300 budget (attempt-1 failure mode on 2026-07-10).
- The hub key needs ~199 CKB spendable **per accepted channel** (§2). The
  script's auto-top-up covers this; if channels stall "pending manual
  acceptance", check the hub log first (attempt-3 failure mode on 2026-07-10).
- Phones: the copy button uses the async clipboard API (secure context — fine
  over the tunnel); if the browser denies it, a prompt with the address pops
  instead.
