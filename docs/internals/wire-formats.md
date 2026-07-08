# Wire formats

Every byte layout that must agree between the **lock**
(`contracts/controller-session-lock/src/main.rs`), the **game type script**
(`contracts/controller-game-cell/src/main.rs`), the **SDK** (`sdk/src/lib.rs`,
`sdk/src/game.rs`), the **wasm surface** (`wasm/src/lib.rs`), and every JS
client. If two of these disagree, funds lock up or forged data validates —
there is no partial failure mode.

**The contract is enforced by tests:** `sdk_sanity` /
`paymaster_sanity` / `service_sanity` / `game_operator_sanity` run SDK-built
bytes through the real on-chain binaries in CKB-VM, and the wasm host-unit
tests match the wasm surface against the SDK byte-for-byte. **Any change to a
layout in this file must change the corresponding contract + SDK + tests in
the same PR, and update this page.**

Conventions: all integers are **little-endian**; `blake2b_256` is CKB's
blake2b (32-byte digest, personalization `ckb-default-hash`); a "pubkey hash"
is `blake2b_256(compressed 33-byte secp256k1 pubkey)[0..20]` (blake160);
signatures are 65-byte recoverable secp256k1 (`r ‖ s ‖ recid`); `‖` is
concatenation.

---

## 1. Account lock script

```
Script {
  code_hash: <blake2b_256 of the lock binary>   (data hash — NOT a type id)
  hash_type: data2  (0x04)                       REQUIRED — see gotcha below
  args:      one of the two layouts in §2
}
```

**`data2` is load-bearing:** the lock is built with RVV/`+zb*` extensions and
calls the `spawn` syscall; both exist only on CKB-VM v2. Referencing it as
`data1` fails with `InvalidEcall(2601)`. (The vendored **ckb-auth** binary, by
contrast, is spawned `data1` from inside the lock — `verify_signature` uses
`ScriptHashType::Data1` with the `AUTH_CODE_HASH` pinned by `build.rs`.)

Full-address encoding (RFC21, `controller_address` in wasm / `addr` in the
CLI): bech32m over `0x00 ‖ code_hash(32) ‖ hash_type(1) ‖ args`, hrp `ckt`
(testnet) / `ckb` (mainnet). Raw hash-type bytes: data=0x00, type=0x01,
data1=0x02, **data2=0x04**.

## 2. Lock args — trust model selected by LENGTH

| len | model | layout |
|---|---|---|
| **20** | authorization-carried | `owner_pubkey_hash(20)` — session params + owner blessing ride in every witness (§5) |
| **116** | registered | `owner_pubkey_hash(20) ‖ session_params(96)` — params baked into the cell by an OWNER tx |

Any other length → `ArgsLenError`. Constants: `OWNER_ONLY_ARGS_LEN = 20`,
`ARGS_LEN = 116` (lock) = `REGISTERED_ARGS_LEN` (SDK).

## 3. Session params (96 bytes)

Offsets as in the lock's `SP_*` ranges; built by `sdk::session_params` /
wasm `session_params`:

| bytes | field | notes |
|---|---|---|
| 0..20 | `session_pubkey_hash` | all-zero ⇒ `SessionDisabled` (no session may ever unlock) |
| 20..28 | `expires_at` (u64 LE, **seconds**) | `u64::MAX` = `NO_EXPIRY` sentinel: the lock skips its header-dep read entirely, so a counterparty-built tx with no header dep (Fiber funding) can be session-signed. Otherwise: tx must carry a header dep, and `header.timestamp_ms / 1000 >= expires_at` ⇒ `SessionExpired`. |
| 28..60 | `policies_root` (32) | Merkle root over allowed policies (§7). `0xFF × 32` = `WILDCARD_ROOT` sentinel: policy check skipped. All-zero = the empty-set root (nothing allowed except the account's own continuation). |
| 60..76 | `spend_cap` (u128 LE, **shannons**) | max net CKB outflow from account cells per tx (§8). `u128::MAX` = unlimited. |
| 76..96 | `guardian_pubkey_hash` (20) | all-zero = no guardian; otherwise a guardian co-signature is REQUIRED in every session witness (§5). |

## 4. Account cell data — revocation epoch

`data[0..8]` = `revocation_epoch` (u64 LE); **empty data = epoch 0**
(`sdk::epoch_data(0)` returns empty bytes; the lock's
`current_revocation_epoch` reads 0 when `data.len() < 8`). Longer data is
allowed; only the first 8 bytes are the epoch.

- Read from the **GroupInput** account cell being spent — a session holder
  cannot forge it.
- Bound into the carried-model owner authorization (§6), so bumping the epoch
  (an OWNER-mode tx that rewrites data) revokes every outstanding carried
  session without changing the address.
- Sessions cannot change it: `enforce_account_outputs` requires any output
  re-creating this lock to keep args AND data byte-identical
  (`SessionCannotAdminister`).

## 5. Witness layout (`WitnessArgs.lock` of the FIRST group input)

First byte = mode; the lock also rejects a second group input up front
(`MultipleInputs` — **one account cell input per tx**, the reason for the
single-cell hygiene in the demo scripts).

```
OWNER    (mode 0): 0x00 ‖ owner_signature(65)
SESSION  (mode 1), registered (args len 116):
                   0x01 ‖ sig_region
SESSION  (mode 1), carried    (args len 20):
                   0x01 ‖ session_params(96) ‖ owner_authorization(65) ‖ sig_region

sig_region = session_signature(65)
           ‖ guardian_signature(65)     iff params.guardian_pubkey_hash != 0
           ‖ proof_region               (§7; empty when policies_root = WILDCARD)
```

Assemblers: `sdk::{owner_witness, session_witness_registered,
session_witness_carried}` (each wraps the payload in a molecule
`WitnessArgs` with only `lock` set).

Signatures verify via **ckb-auth** (`spawn_cell`, algorithm id 0 =
CKB secp256k1) against the message in §6.

## 6. Signing messages (domain-separated, all blake2b_256)

| message | layout | signed by |
|---|---|---|
| **tx message** | `blake2b_256(molecule RawTransaction with cell_deps := [])` | owner (OWNER mode), session key, guardian — per tx |
| **session auth** (carried model) | `blake2b_256("ckb-controller/session-auth/v1" ‖ script_hash(32) ‖ revocation_epoch(8 LE) ‖ session_params(96))` | owner — ONCE per (account, epoch, params); not tx-specific |
| **game intent** (§10) | `blake2b_256("ckb-controller/game-intent/v1" ‖ game_id(32) ‖ player_hash(20) ‖ points(8 LE) ‖ nonce(8 LE))` | the player's session key — per move |

Clearing `cell_deps` in the tx message is what makes the paymaster possible:
a relayer can attach fee cell-deps to an already-signed tx without breaking
the signature. **Corollary: inputs/outputs/witness *structure* are covered —
a signer must session-sign LAST, after assemble-and-balance** (the
assemble-then-sign rule; `paymaster/src/assemble.rs`). `script_hash` in the
session-auth message is the account lock's script hash — the blessing is
bound to one exact account.

## 7. Policy Merkle tree + proof region

**Leaf** = `blake2b_256(script_hash)` where `script_hash` is the allowed
script's own hash (`Script::calc_script_hash`). The same leaf rule serves
both dimensions; the *kind* byte in the proof frame says which hash of the
output to check:

- `kind 0` (`POLICY_KIND_TYPE`) — the output's **type-script** hash. A
  type-less output can never satisfy a type policy (`PolicyNotAllowed`).
- `kind 1` (`POLICY_KIND_LOCK`) — the output's **lock-script** hash. This is
  how a channel session is scoped to "may only fund the Fiber funding-lock /
  settle to the account", and it closes the hole where type-less outputs to
  arbitrary locks were unconstrained.

**Node** = `blake2b_256(min(a,b) ‖ max(a,b))` — sorted pairs, so proofs carry
no direction bits. Odd levels are padded with a **zero node**. Empty leaf set
⇒ all-zero root. Builders: `sdk::{policy_leaf, merkle_root,
merkle_proof}`; on-chain verifier walks `node = hash_pair(node, sibling)` up
the flat sibling list.

**Proof region** (in the witness, only when the root isn't WILDCARD): for
each output that is **not** the account's own continuation, **in output
order**:

```
kind(1) ‖ proof_len(1) ‖ siblings(proof_len × 32)
```

Account self-outputs (same lock code_hash + hash_type) are skipped — they're
governed by §4's continuity rule instead. Missing frame ⇒
`PolicyProofMissing`; short/garbled frame or unknown kind ⇒
`PolicyProofMalformed`; failed membership ⇒ `PolicyNotAllowed`. A
single-leaf tree has `proof_len 0` — the frame is still required
(`kind ‖ 0x00`). SDK framing: `proof_region(&[(kind, proof)])`, wasm
`channel_proof_region`.

## 8. Spend cap semantics

`outflow = Σ capacity(GroupInput account cells) − Σ capacity(outputs with
this exact lock, args unchanged)`, saturating; `outflow > spend_cap` ⇒
`SpendCapExceeded`. Only native CKB capacity is counted (UDT amounts are the
UDT type script's job). Outputs that return capacity to the account must
also keep data identical or they already failed §4.

## 9. Lock error codes (exit codes seen on-chain / in tests)

```
 1 IndexOutOfBound    (also: expiring session + missing header dep)
 2 ItemMissing            10 WrongSessionKey (reserved)
 3 LengthNotEnough        11 SessionDisabled
 4 Encoding               12 SessionExpired
 5 MultipleInputs         13 PolicyNotAllowed
 6 ArgsLenError           14 PolicyProofMissing
 7 EmptyWitnessError      15 PolicyProofMalformed
 8 WitnessLenError        16 SessionCannotAdminister
 9 InvalidMode            17 SpendCapExceeded
                          18 AuthError (ckb-auth refused a signature)
```

## 10. Game cell (aggregator type script)

**Script**: `code_hash` = data hash of `controller-game-cell`, `hash_type` =
`data2`, `args` = `game_id(32)` (any other length ⇒ `ArgsLenError`). The
browser's intent signatures bind this game id (§6) — client and cell MUST
agree on it.

**Tx shapes** (counted over the script group): `(0 in, 1 out)` = genesis —
output data must decode to `seq 0, no players` (empty data allowed) or
`GenesisNotEmpty`; `(1 in, 1 out)` = transition; anything else ⇒
`BadTxShape` (the cell can't be destroyed or split).

**State** (the game cell's data; `GameState::{encode,decode}`):

```
seq(8 LE) ‖ count(4 LE) ‖ count × entry
entry = player_hash(20) ‖ score(8 LE) ‖ nonce(8 LE)      (ENTRY_LEN 36)
```

Empty data ≡ the empty state. Exact length required:
`12 + count×36` or `StateMalformed`.

**Intent** (101 bytes; `Intent::{encode,decode}`):

```
player_hash(20) ‖ points(8 LE) ‖ nonce(8 LE) ‖ signature(65)
```

Signature over the game-intent message (§6), verified on-chain via ckb-auth
per intent. Nonce must be exactly `prev + 1` (first move: `nonce 1`) —
`NonceNotFresh` rejects replays and reorders. `points > 1000`
(`MAX_POINTS_PER_MOVE`, the demo rule) ⇒ `PointsTooHigh`.

**Batch** (the transition's witness payload): `n(2 LE) ‖ n × intent(101)`,
carried in **`WitnessArgs.input_type` of the game cell's input**, addressed
by ABSOLUTE input index (type scripts have no per-group witness indexing —
the script finds its own input by script-hash scan). Exact length or
`BatchMalformed`; absent ⇒ `BatchMissing`.

**Transition rule**: output state must equal `apply_batch(input state,
intents)` exactly — intents applied in order, then `seq += n` — or
`StateMismatch`. The operator provides liveness only; it cannot tamper.

**Game error codes**: 1–4 as §9, then `5 BadTxShape, 6 ArgsLenError,
7 GenesisNotEmpty, 8 StateMalformed, 9 BatchMissing, 10 BatchMalformed,
11 NonceNotFresh, 12 PointsTooHigh, 13 Overflow, 14 StateMismatch,
15 AuthError`. (The live forged-intent rejection logged "error 15" =
AuthError.)

## 11. Cross-implementation matrix (where each format is implemented)

| format | lock / type script | SDK (Rust) | wasm → JS |
|---|---|---|---|
| session params | `parse_session_params` | `session_params` | `session_params` |
| args | `auth()` length switch | `owner_only_args` / `registered_args` | `registered_args` |
| witnesses | `verify_owner` / `verify_session` | `*_witness_*` | `owner_witness`, `session_witness_registered`, … |
| tx message | `tx_message` | `tx_message` | `tx_message` (takes molecule hex) |
| session auth msg | `session_auth_message` | `session_auth_message` | `session_auth_message` |
| epoch data | `current_revocation_epoch` | `epoch_data` | `epoch_data` |
| policy merkle | `merkle_verify`/`hash_pair` | `merkle_root`/`merkle_proof` | `wildcard_root`, `policy_leaf`, `channel_proof_region` |
| game state/intent/batch | `game-rules/` (ONE crate compiled into the type script AND `sdk::game` — cannot drift) | re-export of `game-rules` | `game_encode_intent`, `game_intent_message`, `game_apply` |

JS clients that hand-roll any of these count as implementations too — e.g.
`demo/src/game.ts` and `demo/src/deployed.ts` compute the blake160 pubkey
hash with `@noble/hashes` (personalization `ckb-default-hash`, first 20
bytes) rather than calling wasm. Change them in the same PR, or the sanity
tests will fail.
