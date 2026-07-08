# CKB Controller — Session Lock

**Session-based micropayment rails for on-chain games on CKB.** Authorize a game
once (one pop-up), then it acts on the player's behalf silently — no per-action
signing — within tight, on-chain-enforced limits (expiry, an allow-listed set of
cell policies by **type or lock**, a spend cap, a "may not administer the account"
guard, an optional guardian co-signer, and revocation). Pairs with **Fiber** to
move repeated micropayments off-chain (see `plan.md`).

Mission, architecture, and roadmap: [`plan.md`](./plan.md).

> **Status: complete and live on public CKB testnet.** Both trust models
> (registered / authorization-carried), Merkle policy (type **and** lock
> dimensions), expiry, spend cap, guardian, and carried-model revocation are
> implemented and tested: **48 ckb-testtool integration/sanity tests** (in
> CKB-VM) plus lock/SDK/paymaster host-unit tests. The lock, ckb-auth, and the
> game-cell type script are **deployed on public testnet**, and both live demos
> have run end-to-end there (2026-07-08): a session-signed **Fiber channel**
> funded from an in-browser WASM node (open → pay×5 → cooperative settle), and
> the **multiplayer game aggregator** committing session-signed transitions with
> forged intents rejected on-chain. See `plan.md` for the full log and
> [`demo/README.md`](./demo/README.md) for how to run it.

## Layout

```
contracts/controller-session-lock/   the lock (no_std, ckb-std)
  src/main.rs                         entry + the session checks
  build.rs                            computes AUTH_CODE_HASH from deps/auth
contracts/controller-game-cell/      the game aggregator type script (no_std):
                                      verifies every intent signature (ckb-auth)
                                      and re-derives each state transition
game-rules/                          THE GAME RULES (the template) — one crate
                                      compiled into both the type script and the
                                      off-chain stack, so rules cannot drift;
                                      edit its lib.rs to make your own game
sdk/                                 off-chain builder (controller-sdk, std):
  src/lib.rs                          args/params/Merkle/messages/witnesses
paymaster/                           sponsored-relay core (controller-paymaster, std):
  src/authz.rs                        capability gate (Ed25519 sponsor tokens)
  src/assemble.rs                     assemble-then-sign fee balancing
paymaster-service/                   deployment shell (CkbRpc + SponsorService +
  src/bin/game-operator.rs            tiny_http) + the live game operator bin
wasm/                                controller-wasm (wasm-bindgen browser client)
demo/                                Vite + TS browser demos (channel + game),
                                      mock and LIVE-testnet modes — see its README
tests/                               ckb-testtool integration tests:
  src/tests.rs                        hand-built txs exercising every lock check
  src/game_tests.rs                   game-cell type script transitions
  src/sdk_sanity.rs                   SDK-built txs verified against the real lock
  src/paymaster_sanity.rs             full sponsored-relay handshake, end-to-end
  src/service_sanity.rs               paymaster-service against the real lock
  src/game_operator_sanity.rs         operator batching/finalization
deps/auth                            vendored ckb-auth binary
```

## SDK (off-chain builder)

`controller-sdk` constructs exactly what the lock expects — `owner_only_args` /
`registered_args`, the 96-byte `session_params`, the policy Merkle `merkle_root`
+ `merkle_proof` (sorted-pair blake2b, matching the lock), `tx_message`,
`session_auth_message`, `epoch_data`, and the `owner_witness` /
`session_witness_registered` / `session_witness_carried` assemblers. Signing is
left to the caller (sign the messages the SDK computes, pass signatures back).
The `sdk_sanity` tests run SDK-built transactions through the real lock binary in
CKB-VM, so any drift between off-chain and on-chain is caught.

## Paymaster (sponsored relay)

`controller-paymaster` is the testable core of the gasless-relay server: a
**capability gate** (`authz` — signed, scoped, expiring Ed25519 sponsor tokens,
behind a `Gate` trait so production can swap in `biscuit-auth` as Fiber does) and
**assemble-then-sign balancing** (`assemble` — append the relayer's fee input +
change to the client's partial tx *before* the client session-signs, so the
session signature covers the final tx). The networking/RPC/live-cell-selection/
broadcast shell is deployment glue and intentionally not in the crate. The
`paymaster_sanity` test drives the whole handshake — issue token → gate →
balance → client session-signs (via the SDK) → verify against the real lock in
CKB-VM, with the game paying no gas.

## Channel sessions (Fiber micropayments)

For the high-frequency in-game economy (buy troops, pay-per-move, tips), repeated
transfers belong **off-chain on Fiber**, not on L1. The lock supports this with
**lock-dimension policies**: a session can be scoped so it may only move value
into the Fiber **funding-lock** (open a channel) or back to the account (settle),
bounded by the spend cap (= channel budget). One owner approval then covers both
the L1 state session and the funded channel; the game streams micropayments
off-chain with no popups, settling to L1 only on close. This is implemented,
tested (`channel_session_funds_channel` + reject case), **and proven live on
testnet**: an in-browser Fiber WASM node opened a channel with a session-signed
external-funding tx, streamed payments, and cooperatively settled back to the
account. Full design + roadmap: [`plan.md`](./plan.md); run it via
[`demo/README.md`](./demo/README.md).

## Design recap

- **Trust models (selected by args length):**
  - **Registered** — `args` = `owner_pubkey_hash(20) ‖ session params(76)` = 96
    bytes. The owner baked the session into the cell (one on-chain tx); the lock
    trusts it because only OWNER mode can rewrite args.
  - **Authorization-carried** — `args` = `owner_pubkey_hash(20)` = 20 bytes. No
    on-chain session setup; the session params + an owner signature authorizing
    them ride in the witness and are re-verified every tx.
- **Session params (96 bytes)** (= `args[20..116]` in the registered model):
  `session_pubkey_hash(20) ‖ expires_at(8 LE) ‖ allowed_policies_root(32) ‖
  spend_cap(16 LE u128) ‖ guardian_pubkey_hash(20)`. `spend_cap` = max net CKB
  shannons that may leave the account cells per tx (`u128::MAX` = unlimited).
  `guardian_pubkey_hash` all-zero = no guardian; otherwise a guardian
  co-signature is required.
- **Witness** (`WitnessArgs.lock`): `mode(1)` then
  - `OWNER`: `owner_signature(65)`.
  - `SESSION`, registered: `sig_region`.
  - `SESSION`, carried: `session_params(96) ‖ owner_authorization(65) ‖ sig_region`.
  - **sig_region**: `session_signature(65) ‖ [guardian_signature(65)] ‖
    proof_region` — the guardian signature is present iff the params set a guardian.
  - **proof region**: for each output that is NOT the account's own continuation
    (in output order), `kind(1) ‖ proof_len(1) ‖ siblings(proof_len*32)` — a Merkle
    membership proof that the output is allowed. `kind` = 0 proves the output's
    **type-script** hash; `kind` = 1 proves its **lock-script** hash. The lock
    dimension lets a session be scoped to e.g. "may only fund the Fiber
    funding-lock" (or settle back to the account) — and it closes the prior hole
    where type-less outputs to arbitrary locks were unconstrained.
- **Owner authorization message** (carried model): `blake2b_256(domain ‖
  script_hash ‖ revocation_epoch(8 LE) ‖ session_params)` — bound to this exact
  account, the current revocation epoch, and these params; not tx-specific, so one
  owner signature authorizes the session across many txs.
- **Revocation (carried model)**: the account cell's **data** holds a monotonic
  `revocation_epoch` (first 8 bytes LE; empty = 0). The owner revokes by bumping
  it via an OWNER tx (data changes, address does not); carried authorizations
  signed for an older epoch then fail. Sessions can't alter account data
  (continuity), so they can't reset the epoch.
- **Merkle scheme**: leaf = `blake2b_256(type_script_hash)`; internal node =
  `blake2b_256(min(a,b) ‖ max(a,b))` (sorted pairs ⇒ no direction bits),
  ZERO-padded for odd levels. The off-chain SDK building `allowed_policies_root`
  MUST use this exact scheme.
- **Signing message**: `blake2b_256(raw_tx with cell_deps cleared)` — lets a
  paymaster attach fee cell-deps without breaking the signature
  (technique borrowed from fiber-scripts).
- **Signature check**: delegated to **ckb-auth** via `spawn_cell`
  (algorithm id 0 = CKB secp256k1).

## Building & testing

Prerequisites:

- The CKB RISC-V target: `rustup target add riscv64imac-unknown-none-elf`
  (or `make prepare`).
- A **clang 16+** to cross-compile ckb-std's tiny C stub to riscv64.
  `scripts/find_clang` auto-detects it, including an **Android NDK** clang
  (`$ANDROID_HOME/ndk/*/.../clang`), so no separate install is needed if you
  have the NDK. Otherwise install LLVM, or set `CLANG=/path/to/clang`.
- The **ckb-auth** binary at `deps/auth` so `build.rs` pins its code hash
  (already vendored here; absent → build warns and uses a zero hash).
- For the integration tests only: a host **`gcc`** on PATH (ckb-testtool pulls
  ckb-vm, which assembles its x86 interpreter with gcc). On Windows, e.g.
  `scoop install mingw`.

Build the lock (produces `build/release/controller-session-lock`):

```bash
./build.sh        # portable: needs only bash + rustup + a clang (no make)
# or
make build        # same, via the Makefile
```

Test:

```bash
cargo test -p controller-session-lock   # lock host unit tests (merkle, params) — no toolchain
cargo test -p controller-sdk            # SDK host unit tests (merkle roundtrip, params) — no toolchain
cargo test -p controller-paymaster      # paymaster host unit tests (gate, assemble) — no toolchain
cargo test -p tests                      # integration + SDK + paymaster sanity (needs build/ + gcc)
# or
make unit         # the host unit tests
make test         # the integration + sanity tests
```

Verified on this machine (last run 2026-07-08): `build.sh`/`make build` produce
valid `EM_RISCV` ELFs, and `cargo test -p tests` passes **48/48** tests in
CKB-VM (~1.5M cycles for the happy paths; multi-signature paths higher):

- 19 hand-built lock tests — owner mode, both session models, policy by type
  **and lock** (incl. channel-funding + reject cases), expiry / spend-cap /
  guardian (each with reject cases), account-data continuity, and carried-model
  revocation (epoch bump, re-bless, data-tamper guard);
- 15 game-cell type-script tests (genesis, transitions, forged/replayed-intent
  rejection);
- 4 SDK-built sanity tests proving `controller-sdk` and the lock agree;
- 5 paymaster + 2 service sanity tests (the full sponsored-relay handshake and
  the `gameplay_session_loop`, against the real lock);
- 3 game-operator sanity tests (batching + finalization).

## Off-chain components (built — in this repo)

The off-chain half called for by the two-layer-auth design (doc §9) is now
implemented alongside the lock:

- **Paymaster / relayer service** — `paymaster/` (capability gate +
  assemble-then-sign core) and `paymaster-service/` (HTTP + CKB RPC shell, plus
  the live `game-operator` bin). Sponsorship is gated by signed, scoped,
  expiring capability tokens — not an open relay (the `Gate` trait swaps in
  `biscuit-auth` for prod, as Fiber's RPC auth does). The gasless "open
  transaction" model is design doc §5.
- **Session SDK / client** — `sdk/` (Rust, off-chain builder) and `wasm/`
  (wasm-bindgen browser surface): session keys, the policy Merkle tree/root,
  witnesses, signing messages — driving the browser demos in `demo/`.

## Documentation

- **Building a game on this?** → [`docs/guide/`](./docs/guide/README.md) —
  quickstart (local + testnet), configuration, the session model, writing your
  game's rules, going live, and the trust model.
- **Changing the controller itself?** → [`docs/internals/`](./docs/internals/README.md) —
  architecture, byte-exact wire formats, load-bearing invariants, the
  guarantee→test map, and the deployment runbook. Ground rule:
  [CONTRIBUTING.md](./CONTRIBUTING.md).

## Configuration

One config pair drives every driver (Node scripts, browser demo, game-operator,
and the sibling CLI): **`controller.config.json`** (network, RPC, key file,
game id, session policy, operator/fiber settings) and
**`.controller/manifest.json`** (deployed code cells per network — updated by
the deploy scripts). Env vars override individual values. Byte-level layouts
shared by lock/SDK/wasm/JS are specified in
[`docs/internals/wire-formats.md`](./docs/internals/wire-formats.md) — change a
format there, and the contract + SDK + tests, in the same PR.

## SDK (JS/TS)

`sdk-js/` is `@ckb-controller/sdk` — the runtime a game dev codes against:
`Controller.load({config, manifest, wasm})` → `game().player().move(5n)` for
session-signed on-chain state, `channel({mode})` → `open / pay / close` for the
budget-capped payment rail (mock in-memory or a live in-browser Fiber node).
Witnesses, proofs, funding-tx rules, and every live-mode gotcha live inside;
the demo is its reference consumer. See [`sdk-js/README.md`](./sdk-js/README.md).

## CLI

`cli/` is the developer front door (`node cli/bin.mjs <cmd>`, or `npm link` for
a global `ckb-controller`): **`init`** scaffolds a new project pre-seeded with
the shared testnet code cells (a new game costs ~1k CKB, not a redeploy),
**`deploy`** makes any network game-ready by deploying only what's missing,
**`dev`** boots a full local stack (dev chain + deploys + operator + demo) in
one command, plus **`status`** / **`account grow|drain`** / **`tunnel`**. See
[`cli/README.md`](./cli/README.md).

## Demos

```sh
cd demo && npm install && npm run dev     # mock mode: in-browser only, no funds
```

Three tiers: **mock** (in-browser, no chain, no funds), **local** (dev chain +
local fnn nodes, dev-key funded — runbooks in the sibling repo
`../ckb-controller-cli/`, see its README), and **live** (**public testnet,
actual faucet funds** — the in-browser Fiber channel demo and the multiplayer
game aggregator). For live, follow [`demo/README.md`](./demo/README.md) and
[`demo/GAME-TESTNET.md`](./demo/GAME-TESTNET.md).

## Reference implementations

Modeled on `nervosnetwork/fiber-scripts`:
- `funding-lock` — single-sig unlock, `exec_cell` into ckb-auth, the
  cell_deps-cleared tx message.
- `commitment-lock` — multi-branch unlock, `spawn_cell`+`wait`, output-continuity
  checks, `Since`-based timelocks, UDT awareness.

[ckb-script-templates]: https://github.com/cryptape/ckb-script-templates
