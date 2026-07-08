# Architecture

A session-key **controller** for CKB games: one owner approval authorizes a
temporary **session key**, and from then on the game auto-signs client-side
within on-chain-enforced limits — no per-action pop-up and no per-action owner
signature. The system separates two concerns along a single seam: **state**
moves on L1, **value** moves on L2.

| Rail | What moves | Mechanism | L1 cost |
|---|---|---|---|
| **state** | a game move, a mint, a match result | a session-signed L1 tx that advances a game-state cell | one tx (rare) |
| **value** | buy troops, tips, transfers | off-chain Fiber channel updates, bracketed by an open + a settle | **zero** while the channel is open |

The controller underpins both rails: the same approval authorizes the session
key that signs L1 state txs **and** the budget-capped Fiber channel that carries
the payments. Bounded exposure is the safety property — the channel budget (=
the session's spend cap) is the entire economic exposure of a compromised
session, never the whole wallet.

## The two rails end to end

**State rail** — a player builds a session-signed *intent*, an operator batches
intents into one tx that advances a shared game cell N→N+1, and a **type
script** re-derives the transition and rejects any deviation:

```
player tab (browser)               game-operator (HTTP bin)          CKB L1
────────────────────               ────────────────────────          ──────
game_intent_message()   ──sign──►  POST /intent
  (session key, @noble)            submit(): admission-check vs the
game_encode_intent() ───101B────►    PROJECTED state, then mempool.push
                                   ...
                                   /flush (or timer):
                                     build_transition()  → next_state
                                     encode_batch → WitnessArgs.input_type
                                     finalize: fee (shrink the cell) + lock sig
                                     send_transaction ──────────────► game type script:
                                                                        (1 in,1 out) shape
                                                                        ckb-auth per intent sig
                                                                        apply_batch + equals_unordered
                                                                        ↳ StateMismatch on any tamper
                                     advance tip ◄──── committed ──────
```

The operator has **liveness power only** — it may order or censor, but every
intent is session-signed and the output state is re-derived on-chain, so it can
neither forge a move nor tamper with a score. See
[`paymaster-service/src/operator.rs`](https://github.com/Kagwep/ckb-controller/blob/main/paymaster-service/src/operator.rs)
and [`contracts/controller-game-cell/src/main.rs`](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-game-cell/src/main.rs).

**Value rail** — the session key signs one L1 funding tx (Fiber's
external-funding flow), payments then stream off-chain, and a cooperative close
settles net balances back on L1:

```
              L1 (controller session lock)                 L2 (Fiber, off-chain)
open   ─ session-signs Fiber's external-funding tx ──► funding-lock cell created
         spend cap = budget; a LOCK policy scopes the        │
         session to fund ONLY the funding-lock               ▼
                                                       ChannelReady
pay    ─ (no L1 tx) ─────────────────────────────────► N× send_payment (TLCs),
                                                        tracked off-chain
settle ─ cooperative close tx ───────────────────────► net balances on L1;
         remainder returns to the account cell          the game never signed a per-pay tx
```

The controller authorizes only the L1 funding-lock output (open) within budget
and the settle back to the account; the Fiber node performs the channel logic.
The design reuses CKB *idioms* from `fiber-scripts` (the cell_deps-cleared
signing message, ckb-auth via `spawn`, output continuity) and the
assemble-then-sign pattern from Fiber's funding tx — techniques, not channel
code. See
[`sdk/src/channel.rs`](https://github.com/Kagwep/ckb-controller/blob/main/sdk/src/channel.rs)
for the dev `ChannelSession` API and its in-memory `MockRail`.

## Where authorization vs liveness lives

- **Authorization is on-chain and only on-chain.** The session lock
  ([`contracts/controller-session-lock/src/main.rs`](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-session-lock/src/main.rs))
  enforces session-key signature, expiry, spend cap, policy allowlist, optional
  guardian co-sign, and revocation. The game type script enforces intent
  signatures, nonce freshness, the rule cap, and the exact state transition.
  Break any of these and the tx does not commit.
- **Liveness is off-chain and untrusted.** The operator (state rail) and the
  Fiber node plus paymaster (value rail) provide availability and
  fee-sponsorship. A malicious one of these can stall or censor; it cannot move
  value or forge state beyond what the on-chain scripts already permit.

That division is why a compromised game costs at most the session budget:
everything the off-chain code can do is fenced by the lock's constraints.

## Crate / package map

Rust workspace (rustc **1.85.1**, members in
[`Cargo.toml`](https://github.com/Kagwep/ckb-controller/blob/main/Cargo.toml)),
plus non-workspace JS/TS packages and one sibling repo.

| Crate / package | Single responsibility |
|---|---|
| [`game-rules/`](https://github.com/Kagwep/ckb-controller/blob/main/game-rules/src/lib.rs) (`controller-game-rules`) | The game: state model, wire encodings, the transition function (`apply_intent`/`apply_batch`). `no_std + alloc`. Compiled into BOTH the type script and the off-chain stack — the developer's edit surface. |
| [`contracts/controller-session-lock/`](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-session-lock/src/main.rs) | The L1 **lock**: owner + session auth, policy Merkle, expiry, spend cap, guardian, epoch revocation. `no_std`, runs in CKB-VM. |
| [`contracts/controller-game-cell/`](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-game-cell/src/main.rs) | The L1 **type script**: on-chain framing only (tx shape, witness reading, ckb-auth per-intent sig, `equals_unordered` state check). The rules themselves are `game-rules`. |
| [`sdk/`](https://github.com/Kagwep/ckb-controller/blob/main/sdk/src/lib.rs) (`controller-sdk`) | Off-chain builder: args/params, Merkle root+proofs, witnesses, messages, the `ChannelSession` dev API. `sdk::game` re-exports `game-rules`. |
| [`paymaster/`](https://github.com/Kagwep/ckb-controller/blob/main/paymaster/src/lib.rs) (`controller-paymaster`) | Sponsored-relay core: a capability gate (real `biscuit-auth` + an Ed25519 reference behind one `authz::Gate` trait) + assemble-then-sign balancing. |
| [`paymaster-service/`](https://github.com/Kagwep/ckb-controller/blob/main/paymaster-service/src/lib.rs) | Deployment shell: `CkbRpc` (tip / cell collection / broadcast), `SponsorService`, the `GameOperator`, a `tiny_http` server, and the `game-operator` bin. |
| [`wasm/`](https://github.com/Kagwep/ckb-controller/blob/main/wasm/src/lib.rs) (`controller-wasm`) | `wasm-bindgen` surface over the SDK for the browser: address, args, messages, policy Merkle, witnesses, game intents, `ChannelSession`. Everything crosses as `0x…` hex. |
| [`tests/`](https://github.com/Kagwep/ckb-controller/tree/main/tests/src) | `ckb-testtool` in-VM suites — SDK/paymaster/service/operator-built txs run through the REAL binaries in CKB-VM. |
| `sdk-js/` (`@ckb-controller/sdk`) | TypeScript runtime devs code against: `Controller.load({config, manifest, wasm})`. **Pure logic, injected deps** — the caller passes the parsed config, manifest, and initialized wasm, so one dist runs under Vite and Node. |
| [`cli/`](https://github.com/Kagwep/ckb-controller/blob/main/cli/README.md) (`@ckb-controller/cli`) | Node CLI: `init` / `status` / `deploy` / `account` / `game` / `build` / `dev` / `tunnel`. Wraps the rest into an "install → configure → game runs" tool. |
| [`demo/`](https://github.com/Kagwep/ckb-controller/blob/main/demo/README.md) | Vite + TS browser demos (channel + multiplayer game), the SDK's reference consumer. |
| `../ckb-controller-cli` | Standalone Rust dev-chain / testnet driver. Uses `ckb-types 0.202` and **deliberately avoids `ckb-sdk 5.x`** (which pins `ckb-types 1.1`). Home of the live runbooks (`run-testnet.sh`, `run-channel.sh`, `run-fnn-wss.sh`). |

## Dependency rules (and why)

The purpose of these rules is that **the enforced rules and the client's
precomputation cannot diverge**, and that the lock and type script stay minimal
and auditable.

```
        ckb-hash ─┐
                  ▼
             game-rules ──────────────┐
             (no other deps)          │
              ▲          ▲            ▼
   ckb-std ── │          │ ── ckb-types ── sdk ── wasm ── (JS)
              │          │                  │
      controller-      controller-      paymaster ── paymaster-service
      game-cell        session-lock                    (game-operator bin)
      (type script)    (lock)
```

- **`game-rules` depends on nothing but `ckb-hash`** (plus `alloc`). It is the
  shared source of truth; heavier deps would either bloat the type script or
  block wasm. Verify:
  [`game-rules/src/lib.rs`](https://github.com/Kagwep/ckb-controller/blob/main/game-rules/src/lib.rs)
  imports only `ckb_hash::blake2b_256` and `alloc`.
- **Contracts depend on `game-rules` + `ckb-std`, never on the SDK.** The lock
  and type script must stay `no_std` and CKB-VM-buildable; a dependency on the
  off-chain SDK (which pulls `ckb-types`, std, etc.) would be uncompilable and
  would invert the trust direction.
- **The SDK depends on `game-rules` + `ckb-types`.** It re-exports `game-rules`
  as `sdk::game` (so there is one encoder, not two) and adds off-chain tx
  building on `ckb-types`.
- **wasm depends on the SDK only.** It is a thin hex/JS shim; it adds no logic,
  so the wasm surface is matched byte-for-byte against the SDK in host unit
  tests (see [test-map.md](./test-map.md)).
- **`sdk-js` is pure logic with injected config/manifest/wasm.** It imports no
  chain state and no toolchain — the consumer supplies the parsed config pair
  and the initialized wasm module. This is what lets the same dist run in a Vite
  browser app and in Node.
- **The CLI wraps everything else.** It orchestrates builds, deploys, and the
  operator; it never reimplements a wire format (it calls wasm for those — see
  [`cli/lib/config.mjs`](https://github.com/Kagwep/ckb-controller/blob/main/cli/lib/config.mjs)).

The cross-implementation matrix in
[wire-formats.md §11](./wire-formats.md) lists exactly which function implements
each byte layout on each side. If you change a layout, you change it on the
lock/type-script side, the SDK, the wasm surface, and any hand-rolled JS — in
one PR — or the sanity tests fail.

## Config pair

Everything reads two files at the repository root (environment variables
override individual values everywhere):

- [`controller.config.json`](https://github.com/Kagwep/ckb-controller/blob/main/controller.config.json)
  — network, RPC, key path, game id, session policy (spend cap / expiry /
  policies root / guardian), operator, fiber.
- [`.controller/manifest.json`](https://github.com/Kagwep/ckb-controller/blob/main/.controller/manifest.json)
  — the deployed on-chain artifacts per network (code hashes = data hashes of
  the binaries; deps = the code cells' out-points). These are facts about live
  chains; edit only when (re)deploying. See [deployments.md](./deployments.md).

The account lock is **derived** from the config's session keys plus the
manifest's lock code hash
([`accountLock` in cli/lib/config.mjs](https://github.com/Kagwep/ckb-controller/blob/main/cli/lib/config.mjs)),
verified to reproduce the on-chain cell byte-for-byte — there is no hardcoded
address.
