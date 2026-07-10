# Fiber-central game — design & build plan

> **Status: design proposal, not yet built.** This describes a re-centering of
> the project around Fiber, following the Fiber
> [simple-game](https://www.fiber.world/docs/build/simple-game) model. It is a
> plan, not a description of current behavior. We build on **our own demo**
> (`demo/`), which already consumes `ChannelRail`; the Fiber `simple-game`
> example (`fiber-docs/example/simple-game`) is a *pattern* reference only.

## Motivation

Today Fiber is peripheral: the on-chain game aggregator is the runtime, and the
Fiber channel is an optional side payment. Remove Fiber and the game still runs —
which is the definition of *not central*. The target inverts this:

- **Fiber faces the user and handles gameplay.** A game action *is* a
  micropayment over an open channel; state is local and real-time.
- **CKB is the backend.** Channel funding liquidity, security, and final
  settlement — touched at open and close, not during play.

This is exactly the Fiber simple-game model (each hit pays 1 CKB/point,
off-chain, ~300–500 ms), scaled up with the controller providing the production
layer that model explicitly lacks.

## Governing design principles: multi-user + resilient

Everything is designed for **multiple users** and **connection resilience** from
the start — not retrofitted.

1. **Multi-user by default.** Every user has their own *session* (own keys, own
   account, own spend cap). There is **no shared account**. A "game" is many
   users' sessions converging on shared state. (The single fixed-key account in
   the current demo is a smoke-test shortcut, not the design.)
2. **Resilient to disconnects.** Close / reopen / dropped connections must never
   lose funds or state. Players can leave and rejoin a game.

Mapped onto the two rails — and these requirements *vindicate* the two-rail
design rather than the single-session Fiber-central detour:

| Layer | Multi-user | Resilience |
|---|---|---|
| **Session** (per user) | each user derives their own account from their own keys; identified by session pubkey | session is on-chain-anchored and deterministic from keys → a dropped client re-derives and resumes |
| **State** (game cell) | native: N players' session-signed intents batched onto one shared board | **inherently durable** — state lives on-chain; a reconnecting client just re-reads the current board, nothing lost. Operator = liveness only |
| **Value** (Fiber) | each user funds their own channel to a hub; payments between players route through the hub | **the fragile layer** — channels live off-chain in the browser: needs persistent channel state (IndexedDB), re-peer on reconnect, and on-chain force-close as the backstop |

Synthesis: durable shared state wants the **on-chain aggregator** (a strength
for multi-user, not a liability); resilient value wants **channel persistence +
recovery**. Both rails matter. The Fiber-central single-user demo optimized for
one player's smoothness; multi-user + resilient wants durable on-chain state
*and* recoverable channels.

Foundational moves, in order:
1. **Per-user sessions** — generate keys per browser, derive a per-user account.
   Removes the shared-account blocker; everything else depends on it.
2. **Reconnect + state re-read** — on load/reconnect, re-derive the session and
   read the game cell (state resilience — mostly free, since state is on-chain).
3. **Channel persistence + recovery** — persist Fiber channel state, re-peer on
   reconnect, force-close backstop (value resilience).
4. **Hosted hub + trampoline routing** — an always-on Fiber node acting as a
   **trampoline node**. Lightweight in-browser clients (gossip sync disabled, no
   network graph) only need a path to the *one* hub; the hub does the
   pathfinding to the recipient. **Non-custodial** — TLCs secure the funds, so
   the hub can route but not steal (liveness-only trust, same as the operator).
   This is a first-class, implemented Fiber feature (`send_payment` with
   `trampoline_hops`; feature bit 5; `fiber-lib`), with a working reference in
   `fiber-charge-sim` (browser WASM light clients + server trampoline nodes). It
   is the mechanism for player-A → hub → player-B payments. Plus a hosted app URL.
   Remaining real considerations: hub **liquidity** (needs balanced in/out
   capacity to route), and it's still pre-1.0 (5-hop limit, fee estimation with
   gossip off).
5. **Onboarding funds** — sponsor / faucet / receive-only, so a new user can
   join without a manual funding dance.

## Reference pattern: `fiber-charge-sim` (adopt for multi-user)

A working app that already implements the multi-user, light-client,
trampoline-routed model — the closest blueprint to what we want.

Topology:

```
Browser (each user)              Server (single VPS)
───────────────────              ───────────────────
FiberBrowserNode (WASM,          Router node (fnn) ── the trampoline hub, full graph
  gossip-light, passkey id)       │  everyone channels to it
  │  channel to hub               Station/recipient nodes (fnn) ── payees
  │  sendPayment(                 Next.js API ── issues invoices, runs app logic,
  │    trampoline_hops:[hub])       records payments (SQLite). NON-custodial.
  ▼                               websocat + Caddy ── browser wss → Router tcp
 pays via hub ──────────────────► routes to recipient
```

**Client SDK — `@fiber-pay/sdk/browser`** (higher-level than raw fiber-js):
- `FiberBrowserNode` — `start()`, `connectPeer({address,pubkey})`,
  `listChannels()`, `sendPayment()`, `waitForPayment()`, `shutdownChannel()`,
  `stop()`.
- **`PasskeyCredentialProvider`** — the node identity is a **passkey (WebAuthn)**:
  `register(name)` / `isConfigured()`. Biometric, per-device, no seed phrase. It
  *is* the "per-user session" foundational move, solved.
- Needs `crossOriginIsolated` (COOP/COEP), same as our demo.

**Trampoline payment (the routing):**
```ts
node.sendPayment({ invoice, max_fee_amount, max_fee_rate, trampoline_hops: [routerPubkey] });
await node.waitForPayment(hash, { timeout, interval });
```
The browser needs no network graph — just the router pubkey (the server returns
it with each invoice). That is light-client multi-hop.

**Onboarding / funding:** each browser node exposes a CKB funding address
(`nodeInfo.default_funding_lock_script → address`); the user funds that
(faucet/deposit) to get capacity, then opens a channel to the hub.

**Hosting (the "easy, stable" answer — replaces our quick-tunnels):** one VPS
running everything under **systemd**; **prebuilt Linux `fnn`** from GitHub
releases (no from-source build); **Caddy** for automatic HTTPS (gives the secure
context *and* TLS-terminates wss); **sslip.io** for a free wildcard domain (no
registration); **websocat** as a ws↔tcp proxy so browsers reach the Router.
Players just open the `https://…` URL on any device — zero install.

Mapping to our game:

| fiber-charge-sim | our game |
|---|---|
| FiberBrowserNode + passkey | each player's per-user node/identity |
| Router (trampoline hub) | the multiplayer routing hub |
| Stations (recipients) | game payees — another player's node, or a pot/house node |
| Next.js API (invoices, sessions, SQLite) | our operator/game backend (aggregator, scores) — non-custodial |
| Caddy + sslip.io + systemd + prebuilt fnn | the stable hub + easy hosting |

**Strategic decision this surfaces — the funding model:**
- *fiber-pay model* (what the reference does): the node's **passkey key owns the
  funds directly** — simple onboarding, but **no on-chain spend cap** (the node
  can spend all its channel capacity).
- *controller model* (this repo): a **session key bounded by an on-chain spend
  cap + policy + revocation** funds channels via the session lock — the safety
  property is **bounded loss**.
- For staked / real-value games, bounded loss is the reason the controller
  exists. Synthesis: adopt the reference's topology + SDK + passkey + trampoline
  + hosting, and keep the **controller session lock as the channel funding lock**
  where bounded loss matters — *if* `@fiber-pay/sdk` supports an external/custom
  funding lock (the way fiber-js's `openChannelWithExternalFunding` does).

**Finding (verified against `@fiber-pay/sdk@0.2.3` types):** it does **not**.
`OpenChannelParams` has `shutdown_script` (settlement destination) and
`funding_udt_type_script`, but **no `funding_lock_script`**, and there is **no
`openChannelWithExternalFunding` / `submitSignedFundingTx`**. Channels are funded
from the **node's own (passkey) key** — the controller session lock cannot own
the funding, so `@fiber-pay/sdk` gives **no on-chain spend-cap / bounded-loss**.
(A credential "external funding mode" is hinted at — `getCkbSecretKey()`
returning `undefined` — but it's undocumented and not a custom-funding-lock
path.)

**Resulting recommendation — split the adoption:**
- The **infrastructure pattern is SDK-agnostic — adopt it wholesale**: VPS +
  prebuilt Linux `fnn` + Caddy(auto-HTTPS) + sslip.io + websocat + systemd
  (easy, stable hosting); the hub/trampoline topology; the non-custodial
  invoice-issuing server; and the `localStorage` channel-persistence + reconnect
  reconciliation in `useChannelOpening.ts` (our resilience move #3).
- For the **SDK/funding, stay on `@nervosnetwork/fiber-js`** — it *does* support
  external funding (`openChannelWithExternalFunding`), which is what lets the
  **controller session lock own the channel = bounded loss**. Add per-user key
  generation ourselves. Treat `@fiber-pay/sdk` as an **ergonomics reference**
  (passkey identity, browser-node wrapper), not the adopted SDK, because it
  forecloses the controller's core guarantee.
- **Passkey identity** is still worth stealing later — as the credential that
  unlocks each user's *session* key, independent of `@fiber-pay`.

**Confirmed:** `@nervosnetwork/fiber-js@0.9.0-rc7` (already in the demo) exposes
`trampoline_hops?: Pubkey[]` in `SendPaymentCommandParams` (`payment.d.ts:52`).
So the fiber-js + controller path is **fully viable**: it already has both
external funding (bounded loss) *and* trampoline routing (multi-user light-client
paths).

### Verdict

Stay on **`@nervosnetwork/fiber-js`** and keep the **controller session lock**.
It uniquely gives, together:
- **external funding** (`openChannelWithExternalFunding`) → controller lock owns
  the channel → **on-chain bounded loss** (which `@fiber-pay/sdk` forfeits), and
- **`trampoline_hops`** → multi-user light-client routing through a hub.

Adopt from `fiber-charge-sim` everything that is **SDK-agnostic**: the hosting
stack (VPS + prebuilt `fnn` + Caddy + sslip.io + websocat + systemd), the
hub/trampoline topology, the non-custodial invoice server, and the
`localStorage` channel-persistence + reconnect reconciliation. Borrow the passkey
identity idea later as the unlocker for each user's session key. This path yields
multi-user + trampoline routing + easy hosting + per-user sessions **and** the
controller's bounded-loss guarantee.

## The key realization: Fiber already has the hard parts

The two properties that make in-game payments viable are **inherent to Fiber
during play** — the controller's job is to extend them to the L1 boundary, not
to reinvent them.

| Property | During play (micropayments) | At open / settle (L1) |
|---|---|---|
| **No popup** | inherent to Fiber — off-chain TLCs need no per-payment signature | needs the **session key** — authorize once per session, not per open |
| **Gasless** | inherent to Fiber — off-chain, no L1 fee | needs the **paymaster** — sponsors the L1 open/settle tx |
| **Bounded loss** | the channel cannot spend beyond what it was funded with | the **spend cap** = the funding amount = worst-case loss |

So the micropayments are free and popup-free by nature; what actually needed
authorizing — and would pop a wallet in a naive flow — is the **channel
funding**. The controller's real contribution is:

> **gasless, budget-scoped channel funding + on-chain settlement** — authorize
> once, the game opens a channel funded within the spend cap, plays freely, and
> settles at the end. Worst case = the budget.

The session key does **not** sign every micropayment. It signs only the L1
funding tx (open) and the settle tx (close). This matches the existing
open → pay → close model in `sdk/src/channel.rs`.

## Scope of this first cut

Two decisions were made to keep the first build minimal and faithful to the
reference (both are upgradeable later without re-architecting):

- **Trust: peer-to-peer.** Each node trusts the other's reported game events,
  exactly as the simple-game demo does. State is client-authoritative and
  therefore cheatable — acceptable for a demo-grade first cut, **not** for
  real-value play-to-earn. Upgrade path: an authoritative game-server node, then
  on-chain / TLC-conditional verification.
- **Topology: 1:1 peer.** Direct player-vs-boss channels, the demo's shape. No
  matchmaking, hub, or mesh yet.

## Architecture

```
   USER-FACING (Fiber)                          BACKEND (CKB L1)
   ──────────────────                           ────────────────
   Phaser game                                  session lock
     │  hit / damage event                        │  authorizes funding (open)
     ▼                                             │  + settle (close), gasless
   ChannelRail.pay(n)  ── off-chain TLC ──►        ▼  via paymaster
     │  (instant, gasless, no popup)             funding-lock cell  ◄─ open (1 L1 tx)
     │                                             │
   local score = net channel balance              ▼
     │                                           settle tx  ◄─ close (1 L1 tx)
     ▼                                             │  score = money, so the net
   game over ──────────────────────────────────►  ▼  balance IS the final score
```

Because **score = money** (every point was a payment), "on-chain settlement of
final scores" is simply the cooperative close — no separate payout computation is
needed for the base case.

## The integration seam (our demo)

We extend the existing `demo/` — the SDK's reference consumer, which already
uses `ChannelRail` — rather than porting the Fiber example. Two entrypoints
exist today:

- **`demo/index.html` → `demo/src/main.ts`** — the **channel demo** ("buy
  troops"): `createRail` → `open` / `pay` / `close`, in **mock and live**
  (in-browser Fiber node) mode, already budget-guarded. This is the Fiber-facing
  base to extend.
- **`demo/game.html` → `demo/src/game.ts`** — the **on-chain aggregator**
  scoreboard (the state rail we are demoting).

So most of the payment plumbing is already here. What the Fiber simple-game does
per hit (`payPlayerPoints(...,10)` in its `MainScene`) is what our demo already
does per troop (`rail.pay(COST_PER_TROOP)` in `main.ts`). The remaining work is
turning that economy into a game loop and settling by score.

### Boxes: what our demo already ticks vs. what remains

| Box (from the Fiber game production list) | Our demo today | Remaining |
|---|---|---|
| Channel opening (session-funded, gasless) | ✅ `rail.open()`, mock + live | ✅ 1:1 matching — peer fields in Connect UI |
| Insufficient channel balance | ✅ budget guard + `InsufficientBalance` | surface it in the game UI |
| Security for channel management | ✅ session lock (cap, policy, revocation) | — |
| Real-time gasless microtx, no popup | ✅ off-chain `pay()` | — (inherent) |
| Game loop drives the payments | ✅ `fiber-game.html` — each shot = `rail.pay()` | tune gameplay/economy |
| On-chain settlement of **final scores** at game end | ✅ game-over → `rail.close()` (score = balance) | live-mode confirm |
| Bidirectional pay (both sides earn) | ⛔ one-directional (player spends) | add the earn direction if the game needs it |

**The seam is `demo/src/main.ts`:** replace the manual "buy troops" trigger with
game events calling `rail.pay()`, and fire `rail.close()` on game-over so the
final channel balance settles the score on-chain. The rail, open, budget guard,
and live Fiber node are already in place.

## Reuse vs. build vs. drop

**Reuse (the proven core):**
- The **session lock** — spend cap = budget, LOCK-policy scoping to the
  funding-lock, no-administer, revocation.
- **Gasless open/settle** — the paymaster (`paymaster/`, assemble-then-sign).
- **`ChannelRail` / `ChannelSession`** — the open/pay/close rail and its budget
  guard.
- The **in-browser Fiber node** integration (already proven on testnet).

**Build (small):**
- The **game loop in our demo** — drive `rail.pay()` from game events in
  `demo/src/main.ts` (replacing the manual "buy troops" trigger).
- **Settle-at-game-over** — fire `rail.close()` on game-over; the final channel
  balance is the final score.
- **Session-funded open** — the session lock authorizes the funding tx, gasless
  via the paymaster (largely in place; confirm on live mode).
- **1:1 matching** — connect to the peer (today via `?peer=`), tidied into the
  game's start flow.

**Drop / demote:**
- The **on-chain game aggregator** (`contracts/controller-game-cell`,
  `apply_batch`, per-move L1 transitions). It is the current center and the wrong
  primitive for this model; the operator's role, if kept, shifts from "batch
  intents → L1 transitions" to a Fiber peer/relay.

## Feature coverage

The simple-game readme's "Note for Developers" production list — and the wider
target set — mapped to this plan:

| Feature | Source | This plan | Status |
|---|---|---|---|
| Real-time microtx, no gas | Fiber inherent | `pay()` off-chain TLC | ✅ inherent |
| No popup per action | Fiber inherent | off-chain, session-funded | ✅ inherent + session key |
| Channel opening (with matching) | demo TODO | session-signed `open()`; matching = later | ⚠️ open ✅, matching deferred |
| On-chain settlement of final scores + close | demo TODO | `close()` → settle; score = money | ✅ built |
| Insufficient-balance handling | demo TODO | `remainingCkb()` + `InsufficientBalance` | ✅ built |
| Security for channel management | demo TODO | session lock (cap, policy, revocation) | ✅ built |
| Play-to-earn, instant payments | target | streaming `pay()` + settle | ✅ (P2P-trust caveat) |
| Multi-player token pools | target | needs hub + pool cell | ⛔ deferred |
| Conditional payments on achievements | target | needs TLC/authority | ⛔ deferred |
| Token-based economy / asset trading | target | needs UDT-in-channel | ⛔ deferred |

Most of the list is **inherent to Fiber or already built in the controller** —
the deferred items all require either a hub, an authority/anti-cheat layer, or
UDT-in-channel.

## Open decisions

1. **SDK transport.** Our demo's live rail uses `@nervosnetwork/fiber-js` and is
   **already proven on testnet**; the Fiber game example uses `@ckb-ccc/fiber`.
   Because the rail is SDK-agnostic behind one interface, migrating is contained
   to `LiveRail`. **Recommendation:** stay on `fiber-js` (proven, working) unless
   a concrete `@ckb-ccc/fiber` capability is needed — do not migrate for its own
   sake.
2. **Who funds which side.** The spend cap bounds *funding* (one side's
   liquidity), not each payment direction. For 1:1 player-vs-house, the player
   funds their side within the cap and the house funds its own.

## Build plan (phased)

0. **Spike / derisk.** Confirm the base loop on our demo end-to-end in live mode:
   `rail.open()` (session-funded, gasless) → `rail.pay()` → `rail.close()`. This
   is largely wired already; the spike proves it holds before we add the game.
1. **Game loop** — ✅ done: `demo/fiber-game.html` + `src/fiber-game.ts` (canvas
   shooter) + `src/fiber-game-main.ts` (wiring). Every shot drives `rail.pay()`;
   optimistic fire-and-forget, budget-reserved locally. Typechecks and builds.
2. **Settle-at-game-over** — ✅ done: game-over fires `rail.close()`; the settled
   `remoteCkb` is the score. (Verify on live mode.)
3. **1:1 matching** — ✅ done: a "Play live on testnet" toggle + peer pubkey/WSS
   fields in the Connect step (`fiber-game.html` + `fiber-game-main.ts`),
   prefilled from the URL but UI-driven. No hand-crafted `?live=…` needed.
4. **Polish** — partly done:
   - ✅ **Account pre-flight** (`liveAccountInfo` in `rail.ts`): before a live
     open, reads the account cells via `get_cells` and (a) blocks a multi-cell
     account with a "run `account drain`" message instead of a post-broadcast
     `MultipleInputs`, and (b) clamps the budget below the change-cell minimum
     (~165 CKB reserve) so it can't fail `CapacityNotEnough`.
   - ✅ **Fiber minimum funding** (158 CKB): pre-flight blocks a too-small
     account ("grow it") and raises a sub-minimum budget, so a channel never
     falls below Fiber's `funding_amount` floor.
   - ✅ **Settle TLC race** (`closeWithRetry` + awaiting in-flight pays): the
     first Settle click succeeds even while micropayments are still clearing
     (no more "pending outbound tlcs" needing a second click).
   - ✅ **Actionable error hints** (`withHint`) translate raw lock/fiber errors.
   - **Confirmed live on testnet** (2026-07-09): open clamped to 456 → play →
     settle first-click; settle tx
     `0xdf711f843d390c53d5fe43c20983d9f66c3ad0845ee228125967da8489bc063f`
     committed. Pre-flight also validated against `MultipleInputs`,
     change-cell minimum, and the Fiber funding minimum.
   - remaining: **session teardown** — after settle, open a *new* channel
     without a page reload: `LiveRail.stop()` the in-browser Fiber node and
     reset demo state (`rail`/`handle`/`firedShots`/`score`/`pendingPays`).
     Also: pending/failed channel-state UI, channel recovery (IndexedDB).
5. **SDK decision** — keep `@nervosnetwork/fiber-js` (the demo's live rail) or
   migrate `LiveRail` to `@ckb-ccc/fiber`; bounded either way (see below).

**Deferred (explicitly not in this cut):** anti-cheat / authoritative server,
matchmaking, hub-and-spoke, mesh routing, conditional payments, UDT / asset
trading.

## Risks

- **Fiber maturity.** Building the center on a young network; known rough edges
  already hit (the funding-broadcast quirk, ~300–500 ms TLC latency, tunnel
  churn).
- **Cheatability.** P2P-trust means client-authoritative state — fine for the
  demo, disqualifying for real value until an authority or on-chain verification
  is added.
- **Liquidity.** Both sides must fund their half; a house node needs standing
  liquidity per player channel.
- **SDK migration.** Moving `LiveRail` to `@ckb-ccc/fiber` is bounded but not
  free.

## References

- **`demo/src/main.ts`, `demo/src/rail.ts`** — our channel demo, the base we
  extend (open / pay / close, mock + live)
- `sdk/src/channel.rs`, `sdk-js/src/rail.ts` — the existing rail this plan reuses
- Fiber simple-game guide — https://www.fiber.world/docs/build/simple-game
- `fiber-docs/example/simple-game` — pattern reference (`src/fiber/`,
  `src/scenes/MainScene.ts`)
- `fiber-charge-sim` — the scale-up pattern (in-browser WASM node + server relay)
