# Invariants

The rules that were expensive to learn, promoted out of script headers and
[`plan.md`](https://github.com/Kagwep/ckb-controller/blob/main/plan.md) so they
do not have to be relearned. Each entry states **why** it exists, **where** it
is enforced, and **what breaks** if it is violated. If you are changing
behavior, read this first; if you are adding a rule, also update
[wire-formats.md](./wire-formats.md) and [test-map.md](./test-map.md).

## 1. The lock must be referenced `hash_type = data2`

**Why.** The lock is built with the `+zb*` bit-manipulation extensions and calls
the `spawn` syscall (to run ckb-auth). Both exist only on **CKB-VM v2**, which is
selected by `data2` (raw byte `0x04`). **Where.** The reference is in the account
lock `Script.hash_type`, and every builder hard-codes it: the manifest stores
`"hashType": "data2"`
([`.controller/manifest.json`](https://github.com/Kagwep/ckb-controller/blob/main/.controller/manifest.json)),
`deploy` writes `hashType: "data2"`
([`cli/commands/deploy.mjs`](https://github.com/Kagwep/ckb-controller/blob/main/cli/commands/deploy.mjs)),
and `controller_address` is called with `0x04`
([`cli/lib/config.mjs`](https://github.com/Kagwep/ckb-controller/blob/main/cli/lib/config.mjs)).
**What breaks.** Referencing the same binary as `data1` (VM1) fails at spawn with
**`InvalidEcall(2601)`** — the first issue hit on the live dev-chain run
([plan.md](https://github.com/Kagwep/ckb-controller/blob/main/plan.md), "Live CKB
dev chain"). The game type script is the same: its manifest entries are `data2`
too.

Note the asymmetry: the lock itself spawns the vendored **ckb-auth** binary as
**`data1`** (`ScriptHashType::Data1`, `AUTH_CODE_HASH` pinned by `build.rs`) —
ckb-auth predates VM2. Only the controller scripts require `data2`.

## 2. Exactly ONE account-cell input per tx

**Why.** The lock reads the account cell's data (revocation epoch), sums its
capacity for the spend cap, and treats "the first GroupInput" as *the* account
cell. Two account inputs would make those reads ambiguous. **Where.** The lock's
`auth()` rejects a second group input up front: `if
load_input_since(1, Source::GroupInput).is_ok() { return
Err(Error::MultipleInputs) }`
([main.rs](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-session-lock/src/main.rs)).
**What breaks.** Any tx that spends two cells under the same account lock fails
with **`MultipleInputs`** (exit 5). This is the reason for the
single-account-cell maintenance: a settle leaves the account as two cells
(funding change + settle return), and a naive top-up creates a second cell —
both must be reconciled to ONE cell before the next session tx. The tools that do
it: `account grow` (tops up in place: session witness + one sighash input → one
bigger cell) and `account drain` (session-signs the smallest cell back to the
key). See
[`cli/README.md`](https://github.com/Kagwep/ckb-controller/blob/main/cli/README.md)
and the `grow-account.mjs` / `topup.mjs` note in
[`demo/README.md`](https://github.com/Kagwep/ckb-controller/blob/main/demo/README.md)
("the old `topup.mjs` created a 2nd cell = `MultipleInputs`").

## 3. Assemble-then-sign: session-sign LAST, after balancing

**Why.** The tx signing message is `blake2b_256(RawTransaction with cell_deps
cleared)`
([`tx_message`](https://github.com/Kagwep/ckb-controller/blob/main/sdk/src/lib.rs),
matching the lock's `tx_message`). Clearing `cell_deps` is deliberate: it lets a
paymaster attach fee-related cell-deps to an already-signed tx without breaking
the signature. But **inputs, outputs, and witness structure ARE covered** — the
message commits to everything except cell_deps. **Where.** The paymaster balances
first, then the client signs: `paymaster/src/assemble.rs` appends the fee input +
change output, and only afterward does the client compute `tx_message(&balanced)`
and set its witness (see `paymaster_sanity` / `service_sanity` in
[test-map.md](./test-map.md), which sign *after* `sponsor()` returns). **What
breaks.** Sign before the fee input/change output is added and the message no
longer matches the final tx — `AuthError` on-chain. The only thing a relayer may
add post-signature is `cell_deps`. Corollary for the operator: `finalize` (fee +
game-cell lock sig) must NOT reorder or change output 0, or the tip advance is
wrong
([`operator.rs`](https://github.com/Kagwep/ckb-controller/blob/main/paymaster-service/src/operator.rs)).

## 4. `NO_EXPIRY` exists because Fiber funding txs carry no header dep

**Why.** The lock upper-bounds "now" by reading a **header dep**'s timestamp to
check expiry. But a counterparty-built tx — notably Fiber's external-funding tx —
carries **no header dep**, so the read would fail. **Where.** `expires_at ==
u64::MAX` (`NO_EXPIRY`) makes the lock skip the header-dep read entirely:
`if params.expires_at != NO_EXPIRY { … current_timestamp_ms() … }`
([main.rs](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-session-lock/src/main.rs);
mirrored as `sdk::NO_EXPIRY`). **What breaks.** Without the sentinel,
session-signing a no-header-dep tx fails with `IndexOutOfBound` (exit 1) —
exactly what the old lock did before the live Fiber run forced the fix
([plan.md](https://github.com/Kagwep/ckb-controller/blob/main/plan.md) roadmap
step 3). A `NO_EXPIRY` session is still bounded by spend cap + policy, so it is
not a blanket weakening. Guarded by the in-VM test
`no_expiry_session_unlocks_without_header_dep`.

## 5. The browser funding-broadcast error is expected

**Why.** Fiber's external-funding tx has a **second input** — the acceptor fnn's
reserve contribution, sighash-locked, whose witness the acceptor fills. The
browser node broadcasts its own locally-built copy where `Inputs[1]` is unsigned,
so its `send_transaction` fails **`Inputs[1].Lock` error -11**. **Where / what to
do.** Nothing: **tx hashes are witness-independent**, so the acceptor broadcasts
the fully-signed same-hash tx and the channel proceeds. Do **not** end the
session on that console error (fiber-js rc5 never recovered from doing so; rc7
does). Documented in
[`demo/README.md`](https://github.com/Kagwep/ckb-controller/blob/main/demo/README.md)
("Live-mode issues") and
[plan.md](https://github.com/Kagwep/ckb-controller/blob/main/plan.md) ("LIVE
BROWSER CHANNEL"). Two adjacent issues from the same run: the Cloudflare
quick-tunnel hostname changes every run, and Git Bash mangles multiaddrs
(`/dns4/…` → a Windows path) — invoke node drivers with
`MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*"`.

## 6. Game-cell capacity is state storage — a full cell rejects new players

**Why.** On CKB, a cell's data occupies capacity at **1 CKB/byte**. The game
state grows by `ENTRY_LEN = 36` bytes per player
([`game-rules/src/lib.rs`](https://github.com/Kagwep/ckb-controller/blob/main/game-rules/src/lib.rs)),
so a fixed-capacity game cell has room for a bounded number of players. **Where /
what breaks.** When the cell is full, a transition that adds a player cannot
allocate the larger output and the node rejects it with
**`InsufficientCellCapacity`** (this occurred in the demo: 10 players × 36 bytes
≥ the cell's ~499 CKB —
[plan.md](https://github.com/Kagwep/ckb-controller/blob/main/plan.md)
Direction 1D). **Fix:** `game grow <ckb>` enlarges the cell in place via an
**empty-batch no-op transition** — a `(1 in, 1 out)` transition with zero intents
that only adds capacity, leaving the state byte-identical (guarded in-VM by
`empty_batch_is_a_noop_transition`). Per-game capacity planning is the
developer's responsibility.

## 7. The operator tracks its tip ONLY from its own flushes

**Why.** `GameOperator` advances its `GameTip` to `(tx_hash, 0)` of each tx it
broadcasts
([`operator.rs`](https://github.com/Kagwep/ckb-controller/blob/main/paymaster-service/src/operator.rs)
`flush`). It does not re-scan the chain between flushes. **What breaks.** Any
**out-of-band** transition — for example a `game grow`, or a second operator —
moves the real tip and **orphans** the running operator: its next flush spends a
cell that is no longer live (a `Resolve` error). **Fix today:** restart the
operator so `HttpCkbRpc::find_cell_by_type` re-locates the live tip by type
script at startup
([plan.md](https://github.com/Kagwep/ckb-controller/blob/main/plan.md), "HTTP
OPERATOR"). Auto-rescan on a Resolve error is future work. Practical rule: after
any `game grow` (or any manual transition), restart the operator.

## 8. The game cell can never be destroyed

**Why.** Capacity committed into the game cell is real value; letting a
transition consume it to nothing would burn or leak it, and a "split" would break
the singleton. **Where.** The type script counts its script group and accepts
only two shapes — `(0 in, 1 out)` genesis and `(1 in, 1 out)` transition;
`match (in_count, out_count) { (0,1)=>…, (1,1)=>…, _ => Err(BadTxShape) }`
([game-cell/main.rs](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-game-cell/src/main.rs)).
**What breaks.** `(1, 0)` (destroy) or `(1, 2)` (split) fails with
**`BadTxShape`** (exit 5). Guarded in-VM by `two_output_game_cells_rejected`;
genesis must additionally be empty (`GenesisNotEmpty`) so no pre-credited scores
can be minted.

## 9. Sorted-pair blake2b Merkle with zero padding — identical everywhere

**Why.** The policy allowlist is a Merkle root in the session params; the lock
verifies membership, the SDK builds the root and proofs, wasm and JS mirror both.
If the tree schemes differ by one detail, valid proofs are rejected (funds lock
up) or the shape is exploitable. **Where.** Leaf = `blake2b_256(script_hash)`;
node = `blake2b_256(min(a,b) ‖ max(a,b))` — **sorted pairs**, so proofs carry no
direction bits; odd levels are **padded with a zero node**; the empty set gives
an all-zero root. Implemented in the lock's `merkle_verify`/`hash_pair`
([main.rs](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-session-lock/src/main.rs))
and the SDK's `merkle_root`/`merkle_proof`/`pair`/`fold_level`
([sdk/src/lib.rs](https://github.com/Kagwep/ckb-controller/blob/main/sdk/src/lib.rs)),
wasm re-exports both. **What breaks.** Any divergence (different leaf hash,
unsorted pairs, non-zero padding) → the lock's `PolicyNotAllowed` for txs the
client considers valid. A single-leaf tree has an empty proof but the **frame is
still required** (`kind ‖ 0x00`) — see
[wire-formats.md §7](./wire-formats.md). Full detail is there; this entry exists
so no side is optimized in isolation.

## 10. Revocation epoch lives in account DATA, and sessions can't touch data

**Why.** Carried-model sessions must be revocable without changing the account
address. The revocation epoch is a monotonic counter in the account cell's
**data** (`data[0..8]`, LE; empty = 0), and the carried owner-authorization
signature binds it — bumping the epoch (an OWNER tx that rewrites data) instantly
invalidates every outstanding carried authorization signed for an older epoch.
Data, unlike args, does not change the lock script hash / address. **Where.** The
lock reads `current_revocation_epoch()` from the **GroupInput** cell (a session
holder cannot forge it) and folds it into `session_auth_message`; and
`enforce_account_outputs` requires any output re-creating this lock to keep args
**AND data** byte-identical, or **`SessionCannotAdminister`**
([main.rs](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-session-lock/src/main.rs)).
**What breaks.** If a session could rewrite data it could reset the epoch and
un-revoke itself — so that path is closed; only OWNER mode (which skips the
check) may change data. Guarded in-VM by `carried_session_revoked_by_epoch_bump`,
`carried_session_rebless_after_revocation_unlocks`, and
`session_cannot_alter_account_data`.

## 11. Toolchain pins — each one has a reason

Do not bump these casually; each pin provides a specific compatibility.

| Pin | Where | Why |
|---|---|---|
| **rustc 1.85.1** | [`rust-toolchain.toml`](https://github.com/Kagwep/ckb-controller/blob/main/rust-toolchain.toml) | the workspace baseline every crate builds on. |
| **`idna_adapter = 1.1.0`** | Cargo.lock (pinned via `cargo update`) | `ureq → url → idna` otherwise pulls `icu` crates that need rustc **1.86**; 1.1.0 drops the icu subtree so the service stays on 1.85.1 ([plan.md](https://github.com/Kagwep/ckb-controller/blob/main/plan.md) step 5). |
| **`time = 0.3.41`** | Cargo.lock | `biscuit-auth` pulls `time`; later `time` needs a newer rustc — 0.3.41 keeps the paymaster on 1.85.1 ([plan.md](https://github.com/Kagwep/ckb-controller/blob/main/plan.md) step 4). |
| **`wasm-bindgen = 0.2.126`** + matching CLI | [`wasm/Cargo.toml`](https://github.com/Kagwep/ckb-controller/blob/main/wasm/Cargo.toml), [`scripts/build-wasm.sh`](https://github.com/Kagwep/ckb-controller/blob/main/scripts/build-wasm.sh) | `wasm-bindgen-cli` **must equal the lib version**, and its build deps need a newer rustc — install it with `cargo +stable install wasm-bindgen-cli --version 0.2.126`; the crate itself still compiles on 1.85.1. |
| **clang 16+** for riscv | [`build.sh`](https://github.com/Kagwep/ckb-controller/blob/main/build.sh), [`scripts/find_clang`](https://github.com/Kagwep/ckb-controller/blob/main/scripts/find_clang) | cross-compiling ckb-std's C stub to riscv64. `find_clang` auto-detects it, **including an Android NDK clang**, so no separate LLVM install is needed if you have the NDK. |
| **host `gcc`** for tests | [`README.md`](https://github.com/Kagwep/ckb-controller/blob/main/README.md) build section | `ckb-testtool` pulls ckb-vm, which assembles its x86 interpreter with gcc; the in-VM suites will not build without it (Windows: `scoop install mingw`). |
| **GNU toolchain + mingw OpenSSL** for `fnn` on Windows | `ckb-controller-cli/run-channel.sh` header | Fiber's `cch` dep needs OpenSSL, which has no MSVC C toolchain here — build `fnn` with `cargo +…-windows-gnu` + mingw's bundled OpenSSL 3.6.2 + zlib. |

Reproducibility rests on more than pins: `[profile.release]` in
[`Cargo.toml`](https://github.com/Kagwep/ckb-controller/blob/main/Cargo.toml)
sets `strip = true`, `codegen-units = 1`, and `overflow-checks = true`, which is
why an untouched crate rebuilds to a byte-identical code hash (see
[deployments.md](./deployments.md)).
