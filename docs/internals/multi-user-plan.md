# Multi-user build plan

> **Status: forward roadmap.** The design rationale, trade-off analysis, and the
> `fiber-charge-sim` extraction that this plan rests on are in
> [fiber-game.md](./fiber-game.md). This page is the phased, actionable path.

## Goal

Multi-user game **sessions** on CKB + Fiber: any user, on any OS, opens a URL and
plays with another user. Each user has their own session (own keys, own account,
own spend cap); payments route between users through a hub; connections survive
close / reopen / loss; the server is non-custodial.

## Decisions locked (see fiber-game.md for the reasoning)

- **SDK:** `@nervosnetwork/fiber-js` — it uniquely gives *both* external funding
  (`openChannelWithExternalFunding`) **and** `trampoline_hops`. Not `@fiber-pay/sdk`
  (no external funding → forfeits bounded loss).
- **Funding:** the **controller session lock** owns each channel → on-chain
  **bounded loss** (spend cap + policy + revocation).
- **Routing:** **trampoline** — browsers stay light (no graph); the hub does
  pathfinding. Non-custodial (TLCs secure funds).
- **Infrastructure:** adopt `fiber-charge-sim`'s **SDK-agnostic** parts — hosting
  stack, hub/trampoline topology, non-custodial invoice server, `localStorage`
  channel persistence.
- **Passkey identity:** borrowed later as the unlocker for a user's session key,
  not adopted as the whole SDK.

## Target architecture

```
Browser (per user)                    Server (single VPS)
──────────────────                    ───────────────────
per-user session (own keys)           Hub node (fnn)  ── trampoline, full graph
controller-locked channel  ──────────►  │  everyone channels to it (bounded loss)
  to the hub (session-signed,          Recipient nodes / pot  ── payees
  spend-capped funding)                Non-custodial server ── invoices + game
sendPayment(                             state/rules + results (never custodies)
  trampoline_hops:[hub]) ─────────────► routes payer → hub → payee
                                       Caddy + websocat + systemd + sslip.io
                                         (auto-HTTPS, wss, uptime, free domain)
```

## Already built (reusable building blocks)

From the single-user Fiber-central work (see fiber-game.md build log): the
`ChannelRail` (open/pay/close), the canvas game loop driving `rail.pay()`, the
account pre-flight (multi-cell block, budget clamp, Fiber-min guard), the
settle-TLC-race retry, and the session lock + gasless open/settle. These carry
forward; the multi-user work is mostly *around* them.

## Phases

Each phase lists its goal, work, and a concrete **done-when**.

### Phase 1 — Per-user sessions (the unlock; everything depends on it)

- **Goal:** each browser is a distinct on-chain identity, not the shared fixed-key
  account.
- **Work:** generate + persist a keypair per browser (WebCrypto + IndexedDB/
  localStorage; passkey later). Feed it to `deriveAccount` (which already takes
  keys as input) to get a per-user account/lock. Thread the per-user account
  through `rail.ts` / the game, replacing the fixed `CONFIG.session` path in
  multi-user mode.
- **Done when:** two browsers show two distinct account addresses and each funds
  its own channel independently — no `MultipleInputs` collision on a shared cell.

### Phase 2 — Hub + trampoline wiring (the multiplayer mechanism)

- **Goal:** player A can pay player B, routed through one hub.
- **Work:** stand up one always-on **hub `fnn` node** (the trampoline). Each
  browser opens a **controller-locked** channel to the hub via
  `openChannelWithExternalFunding` (session-signed, spend-capped). Payments:
  `sendPayment({ invoice, trampoline_hops: [hubPubkey], max_fee_amount })`.
- **Done when:** A → hub → B payment completes between two browser nodes on
  testnet, verified by B's balance.
- **Watch:** hub needs balanced in/out **liquidity** (a channel to each player
  with capacity).
- **Run procedure:** [phase2-live-run.md](./phase2-live-run.md) — hub bring-up,
  the acceptor-liquidity knob, per-user funding, and the two-browser driver.

### Phase 3 — Non-custodial game server (the operator)

- **Goal:** a server that runs the game and issues invoices but never holds funds.
- **Work:** server issues recipient invoices (or proxies the recipient node),
  holds game **state/rules**, records results (SQLite/DB). Wire the **state rail**
  — session-signed intents → shared **game cell** (on-chain, durable, multi-user)
  — or server-authoritative state for the P2P-trust MVP (see fiber-game.md trust
  choice). Funds always move over Fiber TLCs, never through the server.
- **Done when:** an N-player match runs end-to-end: each player's actions update
  shared state and payments settle via Fiber.

### Phase 4 — Resilience (survive close / reopen / lost connections)

- **Goal:** a dropped or reopened client resumes without losing funds or state.
- **Work:**
  - **State:** on reconnect, re-derive the session and **re-read the game cell**
    (durable on-chain state — nearly free).
  - **Value:** persist channel state to `localStorage` and reconcile against the
    live channel list on reconnect (the `useChannelOpening.ts` pattern);
    **re-peer** to the hub; **on-chain force-close** as the backstop.
- **Done when:** killing and reopening a browser mid-match resumes the game and
  the channel, with no funds or state lost.

### Phase 5 — Easy hosting (any OS, zero-install players)

- **Goal:** any user opens one `https://…` URL on any OS and plays; host setup is
  one-time and scripted.
- **Work:** single VPS running, under **systemd**: **prebuilt Linux `fnn`** (no
  from-source build), the hub + recipient nodes, the server, **websocat**
  (ws↔tcp), **Caddy** (auto-HTTPS → secure context + wss TLS), **sslip.io** (free
  wildcard domain). Serve the web app with COOP/COEP. Idempotent bootstrap.
- **Done when:** a fresh VPS goes from clone → running via a scripted bootstrap,
  and a phone/other laptop plays over the public HTTPS URL with no install.

### Phase 6 — Onboarding funds

- **Goal:** a brand-new user joins and plays without a manual funding dance.
- **Work:** pick a funding model — guided **faucet**, **paymaster/sponsor** for a
  small starting channel, or **receive-only** via hub inbound liquidity. Add
  **passkey** as the session-key unlocker (borrowed from `@fiber-pay`).
- **Done when:** a new user with no prior setup can get a funded channel and play.

## Deferred (explicitly not in this plan)

Anti-cheat / authoritative-state hardening beyond the P2P-trust MVP; a shared
on-chain **pot/prize** cell settled by final scores (the score-settlement design
in fiber-game.md); UDT-in-channel (token economies / asset trading); mesh /
multi-hub routing; watchtower/dispute automation.

## Cross-cutting risks

- **Hub liquidity** — routing A→B needs the hub funded toward both; ongoing ops.
- **Fiber maturity** — pre-1.0; the rough edges we already hit are handled, but
  multi-hop under real churn and dispute/watchtower robustness are least proven.
- **Onboarding funds** — the genuine UX crux (Phase 6); everything else is
  mechanics.

## Sequencing rationale

Per-user sessions (1) unblock everything. The hub + trampoline (2) is the
multiplayer mechanism. The server (3) makes it a game. Resilience (4), hosting
(5), and onboarding (6) turn it into something any user can actually use. Ship 1
and 2 on testnet first — that proves genuine two-user play — before investing in
3–6.
