# CKB Game Controller — Plan & Architecture

## Mission

**Session-based micropayment rails for on-chain games on CKB:** one approval,
then unlimited smooth in-game value movement — **no per-action signing, no
per-action L1 fee.** Specialized for games (rapid, repeated, low-value moves:
buy troops, pay-per-move, tipping, P2P transfers), but a general session +
micropayment primitive any CKB app can use.

It is a session-based controller for CKB, extended to drive **Fiber** payment
channels so the repeated transfers happen off-chain.

## The two layers

| Activity | Example | Layer | L1 cost |
|---|---|---|---|
| State changes | move, item mint, match result | **CKB L1** — controller session tx | one tx (rarer) |
| Micropayments | buy troops, fees, tips, transfers | **Fiber L2** — off-chain channel updates | **zero** while open |

```
  ONE owner approval  ──►  session key (L1)  +  funded Fiber channel (L2)
        │                        │                       │
        │   state changes        │   micropayments       │
        ▼                        ▼                       ▼
   CKB L1 (rarer)         off-chain, instant, no popup, no L1
   session-signed         (Fiber TLCs, signed by a blessed channel key)
        │                                                │
        └──────────── bracketed by 2 L1 txs ─────────────┘
                  (open/fund the channel  …  settle/close)
```

The **controller is the spine.** One approval blesses *both* a session key (for
L1 state) *and* a budgeted Fiber channel (for the L2 payment rail). After that
everything auto-signs client-side — session key for L1, channel key for payments
— with no popups.

## "Buy troops" walkthrough

1. **Connect (1 popup, 1 L1 tx):** owner approves → session key + a Fiber channel
   funded with, say, 500 CKB.
2. **Play (0 popups, 0 L1):** each "buy troops" is a Fiber micropayment — instant,
   off-chain; troops appear as game state. Repeat thousands of times.
3. **Settle (1 L1 tx):** close the channel; net balances land on L1; if troops are
   on-chain assets, the session-authorized settle mints/checkpoints them.

→ **Thousands of smooth actions bracketed by two L1 transactions.**

## Trust & safety model

- **Bounded exposure:** the channel funding amount is the session's entire
  economic exposure. Worst case (session key / game compromised) the loss is
  capped at the channel balance — never the whole wallet.
- Enforced on-chain by the lock: **spend cap** (max CKB out per tx — the channel
  budget), **policy allowlist** (a scoped session may only create outputs whose
  TYPE or LOCK hash is allowlisted — e.g. only the Fiber funding-lock, or back to
  the account), **no-administer** (a session can't rewrite the account's owner /
  args / data), **expiry**, optional **guardian** co-signer, and **revocation**
  (monotonic epoch in account data, carried model).

## Components & status

| Component | What | Status |
|---|---|---|
| `contracts/controller-session-lock` | CKB L1 lock: owner + session (registered & carried), policy (type **and lock**), expiry, spend cap, guardian, epoch revocation | ✅ built, **live on dev chain** |
| `contracts/controller-game-cell` | CKB L1 **type** script: shared game-state cell, advanced by batched session-signed intents; enforces sig (ckb-auth), per-player nonce (anti-replay), rule cap, exact state transition | ✅ built + 15 in-VM tests |
| `sdk` (`controller-sdk`) | off-chain builder: args/params, Merkle root+proofs, witnesses, messages | ✅ built + sanity-tested |
| `paymaster` (`controller-paymaster`) | sponsored-relay core: capability gate (**real `biscuit-auth`** + Ed25519 reference, behind one trait) + assemble-then-sign | ✅ core built; biscuit gate live |
| `paymaster-service` | deployment shell: `CkbRpc` (tip / live-cell collector / broadcast) + `SponsorService` (gate → collect → balance) + `tiny_http` server (`/health`, `/sponsor`, `/broadcast`) | ✅ built + sanity-tested (mock node, real lock in-VM) |
| `wasm` (`controller-wasm`) | browser/JS client: `wasm-bindgen` surface over the SDK (address, args, messages, policy Merkle, witnesses) + `ChannelSession` open→pay→close; Fiber streaming delegated to fiber-js | ✅ built (wasm32 + JS pkg) + sanity-tested |
| `../ckb-controller-cli` | live dev-chain driver (deploy / addr / play) | ✅ ran a session on-chain |
| Fiber channel integration | session-scoped channels: open (L1) / stream (L2) / settle (L1) | ✅ proven live twice: dev chain (native fnn) AND **public testnet from an in-browser Fiber WASM node** (external funding session-signed, 5 off-chain pays, cooperative settle) |
| Dev `ChannelSession` API | `open` / `pay` / `close` behind a `FiberRail` trait (+ in-memory `MockRail`) | ✅ built + sanity-tested |
| Multiplayer aggregator | `controller_sdk::game` (state/intent model) + `GameOperator` (mempool → batch → transition → broadcast, `game-operator` HTTP bin) + `game.html` browser scoreboard | ✅ built + tested; **proven live on testnet** (deploy + genesis + 2 session-signed transitions committed, 2026-07-08) |

## Built & verified

- **Tests (all green):** 30 ckb-testtool integration (in CKB-VM) + 8 lock host-unit
  + 10 SDK host-unit + 14 paymaster host-unit + 4 paymaster-service host-unit
  + 7 wasm host-unit = **73**. Every lock check has a pass + a reject case; SDK-,
  paymaster-, and service-driven txs are cross-checked against the real lock —
  including the `ChannelSession`-built funding tx, the `NO_EXPIRY` no-header-dep
  path that the live Fiber channel exercised, the real `biscuit-auth` gate
  sponsoring a session tx end-to-end in CKB-VM, the `SponsorService` orchestration
  (gate → collect → balance over a mock node) producing a lock-valid session tx,
  and the WASM surface's outputs matched byte-for-byte against the SDK.
- **Live Fiber channel (CKB dev chain v0.207 + 2× fnn v0.9.0-rc4):** a controller
  session funded a real Fiber channel (external funding, session-signed L1 tx
  committed), streamed 5 off-chain micropayments, and cooperatively settled on
  L1. The session never signed a per-payment tx; exposure was capped at the
  channel budget. See `../ckb-controller-cli/run-channel.sh`.
- **Live CKB dev chain (v0.207, CKB2023):** deployed the lock + ckb-auth, created
  a controller account cell, and **committed a real session-signed action**
  on-chain (account continued, game cell created, account paid its own 1 CKB fee,
  no owner signature). Key gotcha: the lock must be referenced with
  `hash_type = data2` (CKB-VM v2 — has `spawn` + the `+zb*` extensions; data1/VM1
  fails with `InvalidEcall(2601)`).

## Roadmap

1. ✅ **L1 channel authorization** — policy over lock hashes; a session scoped to
   fund only the Fiber funding-lock (or settle to the account), spend cap =
   budget. Tested in-VM (`channel_session_funds_channel` + reject case).
2. ✅ **Dev `ChannelSession` SDK** — `open(budget)` / `pay` / `close`, behind a
   `FiberRail` trait so the FNN is swappable; `MockRail` runs the full
   open→pay×N→close loop in-memory. The L1 funding tx it builds is cross-checked
   against the real lock (`sdk_channel_session_funding_tx`). The "every dev uses
   it easily" surface. Seam left for step 3: `FiberRail::{open,pay,close}` is
   where a real FNN plugs in; the cooperative settle tx is the shape Fiber's
   funding-lock co-signs.
3. ✅ **Real Fiber channel loop** — **DONE, proven live on a 2-node Fiber +
   CKB-dev-chain setup.** Integration = Fiber's **external-funding** flow:
   `open_channel_with_external_funding` takes the controller account as
   `funding_lock_script` (+ lock/auth `cell_deps`) and a `shutdown_script`;
   Fiber returns an unsigned funding tx, the **session key signs it** within the
   spend cap (= budget), then `submit_signed_funding_tx`; micropayments via
   `send_payment`; settle via `shutdown_channel`. Driver: `../ckb-controller-cli`
   `channel` subcommand (reuses controller-sdk for the session sig) +
   `run-channel.sh`. **Live result:** session-signed funding tx
   `0xbb2a87b4…` committed at block 677, 5 off-chain payments (remote balance =
   5×1000 shannons exactly), cooperative close `0xd807786a…` confirmed —
   **1 fund + N off-chain pays + 1 settle.**
   - **Lock change required & made:** added a `NO_EXPIRY` sentinel
     (`expires_at == u64::MAX` ⇒ skip the header-dep read). Fiber's funding tx
     carries no header dep, so the old lock rejected it with `IndexOutOfBound`.
     A no-expiry session is still bounded by spend cap + policy. Covered by
     `no_expiry_session_unlocks_without_header_dep`.
   - **Build:** `fnn` built from source on Windows (GNU toolchain + mingw
     OpenSSL 3.6.2 + zlib, sqlite store). `ckb` v0.207 downloaded; dev chain uses
     Fiber's genesis spec (its scripts in genesis at outputs 5–8, auto-discovered
     by fnn).
4. ✅ **Real `biscuit-auth` gate** — **DONE.** `paymaster/src/biscuit_gate.rs`
   adds `BiscuitAuthority` (issues base64 biscuits) + `BiscuitGate`, implementing
   the same `authz::Gate` trait as the Ed25519 reference, on `biscuit-auth = "6"`
   (the library + version line Fiber gates its RPC with). Tokens carry datalog
   facts `sponsor(scope)` / `subject` / `expires(Date)` + a baked expiry `check`;
   the gate verifies the signature, checks a revocation set, then runs an
   authorizer (`time` fact + `allow if sponsor({scope})`) and reads the verified
   facts back as a `Capability`. Covers offline **attenuation** (holder narrows a
   token, gate honours it) and **revocation** by id — biscuit's headline features.
   7 unit tests + 2 in-VM e2e (`biscuit_gate_sponsors_session_tx`,
   `biscuit_gate_refuses_unauthorized`) cross-check it against the real lock.
   Build note: biscuit pulls `time`, pinned to 0.3.41 (`cargo update`) to stay on
   the workspace's rustc 1.85.1.
5. ✅ **Paymaster service shell** — **DONE.** New `paymaster-service` crate wraps
   the pure core with the deployment layer: a `CkbRpc` trait (tip header, live-cell
   collection via the node's `get_cells` indexer, broadcast) with an HTTP impl
   (`ureq`) + a mock; `SponsorService` orchestrating gate → size fee → collect fee
   cell → assemble-then-balance → broadcast; and a `tiny_http` server exposing
   `/health`, `/sponsor`, `/broadcast` (config via env: CKB RPC, biscuit pubkey,
   scope, fee lock, fee dep). 4 unit tests (mock node) + 2 in-VM integration tests
   cross-check the orchestration against the real lock. The fee cell's own lock
   signature (secp256k1 for a real operator wallet) is the one remaining seam —
   the client's session sig covers the account input only, so the fee witness can
   be filled independently afterwards. Build note: `ureq`→`url`→`idna` pulled `icu`
   crates needing rustc 1.86; pinned `idna_adapter` to 1.1.0 (drops the icu
   subtree) to stay on 1.85.1.
6. ✅ **WASM client** — **DONE.** New `wasm` crate (`controller-wasm`) is a
   `wasm-bindgen` surface over `controller-sdk`, compiling to
   `wasm32-unknown-unknown` (the SDK is wasm-clean: `ckb-hash` auto-uses pure-Rust
   `blake2b-ref` on wasm). Exposes the controller-specific L1 encoding JS CKB libs
   don't provide — `controller_address` (RFC21 bech32m), `session_params` /
   `registered_args`, `tx_message` (from molecule-serialized tx hex),
   `session_auth_message`, the policy Merkle tree, mode-tagged witnesses — plus a
   `ChannelSession` class running the full open→pay→close loop in the browser on
   the in-memory `MockRail`. Everything crosses as `0x…` hex / decimal strings;
   errors as thrown JS strings (so error paths are host-testable too). 7 host
   unit tests verify the surface matches the SDK byte-for-byte. `scripts/build-wasm.sh`
   emits `wasm/pkg/{controller.js,.d.ts,_bg.wasm}` (an ES module a TS app imports).
   **Use, don't duplicate:** off-chain Fiber streaming stays on the JS side
   (`@nervosnetwork/fiber-js`, cf. `fiber-charge-sim`); this crate is L1
   authorization in the browser. Build note: `wasm-bindgen-cli` must match the lib
   version and its build deps need a newer rustc — build it with `cargo +stable
   install`; the crate itself stays on 1.85.1.

## Multiplayer track — aggregator + game cell (shared-state cell)

An account-based chain can let many controllers write one shared game
*contract*, with the sequencer serialising concurrent writes. CKB has no shared
mutable contract storage — a shared cell is one live outpoint, so "everyone
spends it" throttles to ~1 writer/block. The cell-model adaptation is an
**aggregator**:
players submit session-signed *intents*, an operator batches them into one tx that
advances a game-state cell N→N+1, and a **type script** re-derives the transition
and rejects any deviation. The operator has **liveness power only** (order/censor);
it can neither forge a move nor tamper a score — safety is on-chain. This mirrors
the same two-layer split as the payment side: **state** → aggregator + game cell;
**value** → Fiber.

- `contracts/controller-game-cell` — type script. Cell data =
  `seq ‖ count ‖ [player_hash ‖ score ‖ nonce]*`; args = 32-byte game id. Intent
  batch rides in the input cell's witness (`input_type`). Each intent signs
  `blake2b_256(DOMAIN ‖ game_id ‖ player ‖ points ‖ nonce)` (verified via ckb-auth),
  needs a strictly-incrementing per-player nonce (anti-replay), obeys a per-move
  rule cap, and the output state must be exactly `f(input, intents)`. Tx shapes:
  genesis (0→1, must be empty), transition (1→1); anything else rejected. 7 host +
  15 in-VM tests.
- `controller_sdk::game` — std model byte-exact with the script (encode/decode/apply,
  intent message, batch framing); drift-guarded in CKB-VM. 5 tests.
- `paymaster_service::GameOperator` — mempool → `build_transition` → `flush`
  (finalize seam for fee + lock sig, mirrors the paymaster) → broadcast + advance
  tip. Admission-validates intents at `submit` (a doomed intent never jams the
  queue). `game-operator` HTTP bin: `GET /game`, `POST /intent`, `POST /flush`,
  CORS. `CHAIN=mock` runs the whole loop in-memory (no chain/funds). 6 unit tests +
  3 in-VM operator-sanity cross-tests; smoke-tested via curl (2 players, replay
  rejected).
- `wasm` — `game_intent_message` / `game_encode_intent` / `game_decode_state` so a
  browser tab builds + signs an intent. 3 tests.
- `demo/game.html` + `src/game.ts` — multiplayer scoreboard: each tab = a player,
  "Score +5" → session-signed intent → operator → shared board. Typechecks + builds.

**Bring-up scripts — READY + dry-run-validated against live testnet (2026-07-07).**
CCC `.mjs` in `demo/` (each dry-run by default, `send` to broadcast; runbook
`demo/GAME-TESTNET.md`): `game-deploy.mjs` (deploy the type-script code cell),
`game-genesis.mjs` (create the empty game cell), `game-advance.mjs` (commit a
session-signed transition — 2 demo players, intents signed with the browser's
message, next state via wasm `game_apply`, single-input tx that self-funds its
0.001 CKB fee). Dry-runs hit `testnet.ckb.dev`: deploy correctly reports it needs
~68,948 CKB (key has 598 plain), genesis assembles cleanly (input selection + code-
cell-safety assert pass); advance's core is proven offline (getCellLive needs a
real cell).

### ✅ LIVE TESTNET RUN — DONE (2026-07-08)
The aggregator is **proven live on CKB testnet**. Funded via the faucet API
(`POST https://faucet-api.nervos.org/claim_events`, 100k CKB straight to the
sighash `ckt…` address — no Omnilock detour), then:
- **Deploy** `0x2d3cda90…b52bca17b` — game type-script code cell (68,248 bytes,
  68,448 CKB), code_hash `0x81fa44f5…6e330d6c` (data2).
- **Genesis** `0x49dfe452…4c0c02bf` — empty 500-CKB game cell (type script ran
  the genesis path on-chain).
- **Transitions** `0xf211755f…c1fa1b38` (seq 0→2) and `0x5c48548d…2743fe1e`
  (seq 2→4) — each batches 2 session-signed intents (+5 each, nonces 1 then 2);
  the deployed type script verified every intent sig via ckb-auth + re-derived
  the exact transition. Game cell self-funded fees (500→499 CKB), single-input
  txs, code cells never at risk. Current cell: `0x5c48548d…:0`, board decoded
  back from chain (2 players, 10 pts each, nonce 2).

### ✅ HTTP OPERATOR WIRED + LIVE (2026-07-08)
The `game-operator` `CHAIN=http` path is implemented and **proven live on
testnet**: `paymaster_service::sighash::sign_sighash_all` (standard
secp256k1_blake160 finalize over witness 0, preserving the `input_type` batch),
`GameOperator::with_fee` (transition self-funds by shrinking the game cell —
single-input tx, code cells never at risk), `HttpCkbRpc::find_cell_by_type`
(locates the live tip by type script at startup), and queue-shedding on node
rejection (a forged-sig intent passes the sequencer but is rejected on-chain;
the operator drops the batch so it can't jam later flushes). Config:
`CHAIN=http RPC=… KEYFILE=… DEPLOY_FILE=demo/game-deploy.json` (+ optional
`AUTH_DEP`/`SECP_DEP`/`FEE`, defaulting to the testnet deploys).
**Live run:** operator booted on the real tip (seq 4), then two fresh players
posted intents through `/intent` (`demo/post-intent.mjs`, the exact `game.html`
client path): transitions `0xd7a57b21…5f39de4e` (seq 4→5) and
`0xa0038790…c55e96f5` (seq 5→6) committed; a forged-signature intent was
rejected by the deployed type script (error 15) and shed, and the next valid
intent went through. `game.html?operator=http://127.0.0.1:9944` now drives the
real chain.

### NEXT STEP — polish / stretch
- Browser click-through of `game.html` against the live operator (the client
  path is already live-proven via `post-intent.mjs`; this is UX verification).
- ~~Fiber live browser channel re-run~~ ✅ DONE 2026-07-08 — see the Fiber demo
  section: full open→pay×5→settle from the in-browser WASM node on testnet.

## NEXT PHASE — from research repo to developer tool (two directions)

Everything above works, but only for the person who built it: env-var sprawl,
hand-run bash runbooks, hardcoded deploy points (`demo/src/deployed.ts`, the
`ACCOUNT_LOCK` literals in the `.mjs` scripts), a riscv+clang+mingw toolchain,
and tribal knowledge living in script headers and this file. The next phase has
**two directions, worked in parallel**: (1) a **tool** a game dev installs and
configures — create and run a game without touching controller internals, and
(2) **in-depth documentation** — without which the tool can't be maintained by
anyone else (or trusted by its users).

### Direction 1 — the tool ("install → configure → game runs")

**Acceptance test (the whole phase in one sentence):** a dev with Node + a
browser and **no Rust toolchain** runs `npm create ckb-game`, edits a config,
runs `dev`, and is playing a local game (session-signed state moves + channel
payments) in under 15 minutes; `deploy --network testnet` + one faucet claim
puts the same game live. The dev never sees witnesses, Merkle proofs,
account-cell hygiene, or tunnels — only their game rules and a session policy
(budget / expiry / what the session may touch), and the guarantee that moves
and payments **are valid only within that session's limits**.

Build order (A→B are plumbing over what exists; C is repackaging the demo;
D is the only new engineering):

- **A. One config + manifest, kill the env vars.** ✅ **DONE 2026-07-08.**
  `controller.config.json` (network, RPC, key path, game id, session policy,
  operator, fiber) + `.controller/manifest.json` (deployed out-points + code
  hashes per network; `game-deploy.mjs send` updates it). Everything reads
  them, env vars still override: the `.mjs` drivers (via
  `demo/controller-config.mjs` — the account lock is now DERIVED from config
  keys, verified identical to the on-chain cell), the browser (via
  `demo/src/config.ts`, static Vite JSON imports + `fs.allow`), the operator
  (`CONTROLLER_CONFIG`, default `controller.config.json` — a bare `cargo run
  --bin game-operator` is now fully live), and the CLI (falls back to
  `../ckb-controller/controller.config.json`; bare `controller-cli addr`
  reproduces the deployed address). Verified live: intent committed on
  testnet through the zero-env operator (seq 13→14, `0x352e264a…`);
  `game-advance.mjs` now locates the game cell by type script instead of the
  stale `game-cell.json`. Wire-format doc:
  [`docs/internals/wire-formats.md`](../ckb-controller/docs/internals/wire-formats.md)
  ✅ (first Direction-2 internals page — byte-exact args/params/witness/
  Merkle/message/game layouts + both error-code tables, sourced from the
  contracts).
- **B. `@ckb-controller/cli`** ✅ **DONE 2026-07-08** (`cli/` — Node, docs in
  `cli/README.md`). Commands, each verified live:
  - `init` — scaffolds a project with fresh game id + fresh keys + a manifest
    **pre-seeded with the shared public testnet code cells**
    (`lib/known-deployments.mjs`), so a new game costs ~1k CKB, not a ~314k
    redeploy. Verified: scratch project init → status all-green → deploy
    dry-run planned exactly "new game cell + new account".
  - `status` — chain tip, code-cells live?, account single-cell?, game seq,
    operator health. Verified against live testnet, all green.
  - `deploy [--send]` — deploys ONLY what's missing (code cells → game-cell
    genesis → account), plain-cell-only inputs (never risks a code cell),
    updates the manifest. Idempotent on testnet (all-skipped); the
    deploy-missing path fully exercised by `dev` on a fresh devnet.
  - `account show|grow|drain` — hygiene, dry-run default; `drain` refuses
    over-cap amounts BEFORE broadcast (would be SpendCapExceeded on-chain).
  - `dev` — **the 15-minute promise, locally**: boots a dev chain under
    `.controller/devnet-chain` (CKB2023 at epoch 0 → data2/VM2, cellbase
    maturity 0, IntegrationTest `generate_block` for instant commits), funds
    from the well-known dev key, reuses the SAME deploy core with a CCC
    script registry pointed at the devnet secp dep group (discovered from
    genesis block 0 and written to the manifest — the operator needs it
    too), then starts game-operator (`NETWORK=devnet` env override added)
    + vite and prints the play URL. **Verified end-to-end:** cold boot →
    lock/auth/game deployed → genesis committed (the data2 type script RAN
    on the local chain) → account created → operator up → session-signed
    intent → transition `0x0525826e…` COMMITTED on the local chain (seq
    0→1). Re-run is idempotent (~25 s, all deploys skipped; chain state
    persists). Ctrl-C tears the stack down.
  - `tunnel` — wraps `run-fnn-wss.sh` for the browser demo's live mode.
  - The wasm pkg is bundled in `cli/pkg` so standalone projects need no Rust
    toolchain (repo checkouts prefer their own `wasm/pkg`). Still open from
    the original 1B sketch: prebuilt LOCK binaries shipped in the package +
    the Docker reproducible-build image (today `deploy` builds from source
    when a binary is missing) — folds into 1D's toolchain work.
- **C. `@ckb-controller/sdk`** ✅ **DONE 2026-07-08** (`sdk-js/` — TS → tsc →
  ESM dist; docs in `sdk-js/README.md`). The runtime devs code against:
  `Controller.load({config, manifest, wasm})` →
  `game().player().move(points)` (state rail: sign → operator → type script)
  and `channel({mode: "mock"|"live"})` → `open(budget) / waitReady / pay /
  close` (value rail: MockRail in-memory or LiveRail = the in-browser Fiber
  WASM node). Design: **pure logic, injected deps** — the caller passes the
  parsed config pair and the INITIALISED wasm module, so the same dist runs
  under Vite and Node; fiber-js is an optional peer dep, lazily imported only
  by LiveRail. Hides: witness/proof assembly, the cell_deps-cleared message,
  account derivation, Fiber's witness-only funding contract, the
  funding-broadcast red herring, peer/channel-readiness polling, per-player
  nonce rollback. **Dogfooded:** `demo/src` rewritten on it —
  `controller.ts`/`live.ts`/`funding.ts`/`deployed.ts` deleted; `game.ts` and
  `rail.ts` are now thin UI adapters (Vite needs `resolve.dedupe` for the
  file-linked shared deps). **Verified end-to-end (all on 2026-07-08):**
  Node smoke — account derivation matches the deployed address; a
  `player.move(5)` committed on testnet via the live operator (seq 14→15,
  `0x61f08849…`); MockRail loop arithmetic exact (spent 15 / local 85 /
  remote 25··15). **Live browser channel through the SDK:** headless
  `run-live.mjs` over the WSS tunnel — funding `0x02583c03…` (red herring
  handled), ChannelReady, 5×5 CKB off-chain buys, cooperative settle
  committed (`0xc55e23bb…`, 474 CKB back to the account), then the CLI's
  `account drain --send` restored the single-cell invariant
  (`0x91289f0a…`). Still open (folds into later phases): auto grow/drain
  inside the SDK, epoch-revocation helpers, a `startSession()` that MINTS new
  sessions (today the session comes from config; minting = owner-signed
  registration / carried blessing).
- **D. Game rules as the dev's code.** ✅ **DONE 2026-07-08** (template guide:
  `game-rules/README.md`). The rules now live in ONE crate,
  **`game-rules/`** (`controller-game-rules`, no_std + alloc, pure-Rust
  blake2b), compiled into BOTH the type script (`contracts/controller-game-cell`
  now contains only the on-chain FRAMING: tx shape, witness reading, ckb-auth
  sig checks, `equals_unordered` state comparison, error-code mapping) and the
  off-chain stack (`sdk::game` is a re-export → wasm → browser/operator) —
  drift is now structurally impossible, not just sanity-tested. The dev edits
  the marked GAME RULE section (`PlayerEntry`/`GameState`/`Intent`/
  `apply_intent` + rule constants); the nonce anti-replay and batch framing
  stay. **Toolchain:** `ckb-controller build` compiles contracts + wasm and
  syncs `wasm/pkg` → `demo/pkg` + `cli/pkg`; `--docker` runs the contract
  build in `docker/build.Dockerfile` (image authored but UNVERIFIED — no
  docker on this box; the native path is verified). `deploy` now compares the
  local binary's data hash against the manifest and redeploys when the code
  changed — proven by dry-run: the untouched lock rebuilt to the IDENTICAL
  hash (skipped), the refactored game script was flagged as new code
  (`0xc8c98a78…`, vs deployed `0x81fa44f5…`). **Verified:** 8 game-rules host
  tests + 10 SDK + 11 wasm + **48/48 in-VM** against the rebuilt binaries;
  live testnet still accepts new-wasm intents (seq 16→17, `0xff5d03e4…`).
  The old deployed script stays canonical for the demo game (behaviorally
  identical refactor — no redeploy needed).
  - **Found & fixed along the way:** the demo game cell was FULL (10 players
    × 36 bytes ≥ its 499 CKB) — every new player failed with
    `InsufficientCellCapacity`. New `ckb-controller game show|grow`: grow =
    an **empty-batch no-op transition** that only adds capacity (499→999,
    `0xc428a05c…` committed). Corollary: the operator tracks its tip from its
    own flushes, so an out-of-band transition orphans it — restart it to
    re-locate (future: re-scan by type script on a Resolve error).
  - Fully generic rule hosting (interpreted state machines) stays out of
    scope; per-game capacity planning + operator auto-recovery fold into the
    docs/guide work.

### Direction 2 — in-depth documentation (maintainers AND users)

✅ **DONE 2026-07-08.** Both trees written and reviewed (drafted by Opus
subagents from detailed briefs, then fact-checked page-by-page against the
code):
- **`docs/internals/`** — README index + `architecture.md` (two rails, crate
  map, dependency rules), `wire-formats.md` (earlier), `invariants.md` (11
  load-bearing rules with why/where/what-breaks incl. the toolchain-pin
  table), `test-map.md` (guarantee → test across both harnesses; in-VM count
  verified = 48), `deployments.md` (build/reproducibility/costs/roll runbook;
  flags the manifest ↔ `known-deployments.mjs` duplication as a known wart;
  Docker image marked UNVERIFIED).
- **`docs/guide/`** — README index + `quickstart.md` (local + testnet
  tracks), `configuration.md`, `sessions.md`, `your-game.md`,
  `going-live.md`, `trust.md` (three enforcement tiers + attacker table).
- **`CONTRIBUTING.md`** — the mechanics rule: a PR changing a wire format or
  invariant must update every implementation + the internals page + the tests
  in the same PR; checklists per change type.
- Review fixes worth remembering: the guide originally implied epoch-bump
  revocation applies to ALL sessions — corrected (epoch revocation =
  **carried** model; the default registered model revokes via an owner-mode
  re-parameterization, which CHANGES the address); missing-guardian error is
  `WitnessLenError` not `AuthError`; scaffolded projects must point the
  operator at their config via `CONTROLLER_CONFIG` (the repo-root default
  reads the repo's own game id); `game.html` needs the Vite server + an
  explicit `&game=` for non-default ids. The deployments page surfaced a real
  discrepancy: `run-testnet.sh`'s quoted capacities differ from the actual
  binary sizes — the `ls` sizes are ground truth.

Two audiences, two docs trees; the original sketch follows.

- **Maintainer docs (`docs/internals/`)** — so another dev can safely change
  the code: (1) architecture tour — the two layers, every crate and what may
  depend on what; (2) **wire-format reference** — args layouts (20/96/116),
  the 96-byte session params, witness modes + sig_region + proof region,
  the sorted-pair Merkle scheme, the cell_deps-cleared signing message, epoch
  data — the invariant being that lock, SDK, wasm, and type script encode
  these identically (today that contract is enforced only by the sanity
  tests); (3) **invariants & gotchas** promoted out of script headers and this
  plan into a maintained page (`data2`/VM2, single-account-cell, the
  assemble-then-sign order, `NO_EXPIRY`, the funding-broadcast red herring,
  toolchain pins and why each exists); (4) test map — which test guards which
  guarantee, so a red test tells you what you broke; (5) release/deploy
  runbook — how to rebuild binaries reproducibly, verify code hashes, and
  roll the shared testnet deployment.
- **User docs (`docs/guide/`)** — for game devs on the tool: quickstart
  (the 15-minute path), the config reference, the session model *as a user
  concept* (what a budget/expiry/policy means, what happens at the limits,
  how revocation works), "writing your game's rules" (the template's
  `apply()`), going live on testnet, and a **trust page**: what is enforced
  on-chain vs by the operator vs by the SDK — why a compromised game costs at
  most the session budget. Written against the SDK surface from Direction 1C,
  so it stays honest.
- **Mechanics:** docs live in-repo and are versioned with the code they
  describe; every PR that changes a wire format or invariant must touch the
  corresponding internals page (checklist in CONTRIBUTING). This plan file
  stays the project log; the docs become the stable reference.

### NEXT — session minting (a later session)

Today every session comes from config: `init` writes fixed owner/session
privkeys and the account lock is derived from them — fine for demos, but a
real game mints **per-player sessions at runtime**. This is the deferred
`startSession()` from Direction 1C. What it needs:

- **SDK `controller.startSession({budget, expiry, policies, guardian?})`** —
  generate a fresh session keypair, build the 96-byte params, then mint by
  model:
  - **registered**: build + owner-sign an OWNER-mode tx that re-creates the
    account with the new params baked into args (address CHANGES — the SDK
    must surface the new address and re-locate the cell);
  - **carried**: no tx — owner-sign `session_auth_message(script_hash, epoch,
    params)` (all pieces already exist: `sdk::session_auth_message`,
    `epoch_data`, `session_witness_carried`; wire-formats §5–6) and hand the
    blessing to the session holder. Cheaper + revocable-by-epoch, so likely
    the default for per-player minting.
- **Owner-signer seam** — minting is the ONE interactive owner touchpoint, so
  the SDK needs a `sign(message) -> 65 bytes` callback instead of a raw
  privkey (the hook where a wallet / passkey / JoyID signer plugs in later).
- **Revocation helpers** — `controller.revoke()`: owner-mode epoch-bump tx
  (carried) / re-parameterization (registered); plus `session.limits()` for
  UI display.
- **CLI `session mint|revoke|show`** — the scriptable counterpart; update
  `docs/guide/sessions.md` + wire-formats cross-refs when built.
- Groundwork that already exists on-chain: both trust models + epoch
  revocation are implemented and in-VM-tested in the lock (nothing on-chain
  should need to change).

## Demo — testnet L1 + in-browser Fiber WASM node

The roadmap code is done; the demo ties it together on public networks. Two
findings from Fiber's docs shape it:

- **Fiber has a real in-browser node.** `@nervosnetwork/fiber-js` (built on
  `fiber-wasm`) runs an actual Fiber node *in the page* — Web Workers + IndexedDB +
  SharedArrayBuffer, peers over WSS, channels, invoices, payments. So "the node
  lives in the web app" is real, not a thin client to a server FNN. (Long-lived /
  always-online receiving still wants a native `fnn`.)
- **`fiber-js` exposes `openChannelWithExternalFunding()` + `submitSignedFundingTx()`
  in the browser** — the exact flow our controller uses, with the rule "the signer
  may only fill witnesses, never change inputs/outputs/order." Fiber's own WASM
  docs *recommend* external funding so the user's wallet key is never handed to the
  Fiber node. **That recommended-but-unbuilt authorization layer is precisely the
  controller**: the budget-capped session key signs the funding tx instead.

**Where we stand vs Fiber's `simple-game` tutorial** (Phaser shooter, `fiber-docs/
example/simple-game`): that demo is raw Fiber payments — both nodes hold full keys
in the clear, no session/authorization, settlement + security left as TODO. In it,
**movement is local game state; only value-events (hit boss / get hit) are Fiber
payments** — i.e. Fiber's own demo does the value/state split our design assumes.
We are the security layer it lacks: external-funding signed by a bounded session.
Tackled vs theirs: external-funding auth ✅, bounded exposure (cap+policy+expiry+
guardian+revocation) ✅, on-chain settlement/close ✅, insufficient-balance ✅,
gasless L1 ✅. Still unbuilt (theirs too): matchmaking, multi-player pools,
conditional/HTLC payments, UDT trading, passkey/JoyID owner-signer.

**Demo narrative:** *buy troops* (value-events on Fiber), not *move left/right*
(local) — the axis where we beat the tutorial: "the Fiber node runs in your
browser, yet a compromised game loses at most the channel budget, never your
wallet." Bracket = 1 L1 fund (session-signed external funding) + N off-chain pays
+ 1 L1 settle.

**Testnet L1 milestone (in progress):** the CLI driver (`../ckb-controller-cli`)
is testnet-ready — `addr` derives the `ckt…` account address (secp self-test
passes), `play` submits a session-signed action over any RPC. Runbook:
`run-testnet.sh` (deploy lock + ckb-auth → create account → session-signed play).
**Capacity reality:** on-chain storage is 1 CKB/byte, so the code cells are the
cost — lock ≈ 94,876 CKB, auth ≈ 151,004 CKB, account ≈ 400 CKB (~246k total; a
faucet claim is ~300k). Reuse an existing testnet ckb-auth deploy to skip ~151k.
Needs a funded testnet key (faucet) — the on-chain run is the operator's to
execute.

**Browser demo (built, runs now):** `demo/` is a minimal Vite + TS "buy troops"
app over `controller-wasm`. **Mock mode works today** with no node/funds — real
controller address, real session signatures (`@noble/curves` recoverable
secp256k1), real funding/settle tx shapes, the open→pay→close loop on the wasm
in-memory rail; connect → buy troops (off-chain, gasless, no popup) → settle.
Verified: typechecks, `vite build` bundles the wasm, and the open→pay→close path
executes at runtime. **Live mode** (`demo/src/live.ts`) scaffolds the in-browser
Fiber WASM node + external funding signed by the session key — needs
`@nervosnetwork/fiber-js`, testnet config, a WSS peer, and a deployed lock
(COOP/COEP headers already set in `vite.config.ts`).

**✅ LIVE BROWSER CHANNEL — DONE (2026-07-08, public testnet).** The full loop ran
headless end-to-end from a real **in-browser Fiber WASM node** (fiber-js
0.9.0-rc7) against our testnet `fnn` over the WSS tunnel: **1 session-signed
external-funding L1 tx (`0x4dc65468…`, committed) → ChannelReady → 5 off-chain
buys (5 CKB each, no popups, no L1) → cooperative settle (`0x3c463e18…`,
committed): 124 CKB to the fnn (its 99 reserve contribution + the 25 paid),
474.99 CKB back to the account.** Driver: `demo/run-live.mjs` (puppeteer-core +
system Edge, persistent profile). What it took after the 2026-06-30 fixes:
- **fiber-js rc5 → rc7** to match the native fnn build (repo `3c25bcf`, Jul 4).
- **The browser's own funding-tx broadcast error is a RED HERRING**: the funding
  tx carries a 2nd input (the acceptor fnn's reserve contribution, sighash-locked,
  witness filled by the acceptor); the browser's local `send_transaction` fails
  `Inputs[1].Lock` error -11, but tx hashes are witness-independent — the acceptor
  broadcasts the fully-signed same-hash tx and the channel proceeds. Don't kill
  the session on that console error (rc5 never recovered; rc7 does).
- **Git Bash mangles multiaddrs** (`/dns4/…` → `C:/Program Files/Git/dns4/…`) —
  invoke with `MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*"`.
- **`demo/grow-account.mjs`** — top up the account IN PLACE (session witness +
  one sighash input → single bigger account cell; inflow, cap untouched). The old
  `topup.mjs` created a 2nd cell = `MultipleInputs`. After every settle the account
  is 2 cells (funding change + settle return) — drain before the next run.
- **Orphaned channel recovery**: fnn `shutdown_channel force=true` sweeps to a
  CommitmentLock cell (`0x9d1c63ff…`); funds return only after the commitment
  delay. Use a persistent browser profile so channel state survives.

## Relationship to Fiber — use, don't duplicate

Fiber is the **L2 payment layer** (channels, TLCs, routing, invoices); the
controller is **L1 authorization**. We borrowed CKB *idioms* from `fiber-scripts`
(tx-message-with-cell_deps-cleared, ckb-auth via `spawn`, output continuity) and
the assemble-then-sign pattern from Fiber's funding tx — techniques, not channel
logic. Integration point: the session authorizes the L1 funding-lock output
(open) within budget and the settle back to the account; a Fiber FNN handles the
off-chain payment streaming. We never reimplement channels.

## Repo map

```
ckb-controller/            (this repo — workspace, rust 1.85.1)
  contracts/controller-session-lock/   the lock (no_std, ckb-std; build via build.sh/make)
  sdk/                                 controller-sdk (off-chain builder)
  paymaster/                           controller-paymaster (gate + assemble-then-sign)
  paymaster-service/                   deployment shell (CkbRpc + SponsorService + tiny_http)
  wasm/                                controller-wasm (wasm-bindgen browser/JS client)
  demo/                                Vite + TS "buy troops" browser demo (mock + live-scaffold)
  tests/                               ckb-testtool integration + sdk/paymaster/service sanity
  docs/ (in the ckb repo)              controller-session-lock.md (design)
  deps/auth                            vendored ckb-auth binary
../ckb-controller-cli/     (standalone — live dev-chain driver; ckb-types 0.202, no ckb-sdk)
```

(See also the `ckb-game-controller` Claude skill for the cross-repo reference map.)
