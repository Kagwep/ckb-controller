# Test map

When a test fails, this page identifies which guarantee broke. When you add a
guarantee, it identifies where to put the test. Every load-bearing rule has a
**pass** case and a **reject** case; SDK-, paymaster-, service-, operator-, and
wasm-built bytes are cross-checked against the REAL on-chain binaries, so drift
between an off-chain builder and the lock/type-script surfaces as a failure here.

## The two harnesses

**Host unit tests** (`#[cfg(test)]` modules, run with `cargo test -p <crate>`) —
pure Rust, no CKB-VM, no built binaries. Fast; they cover math, encodings, and
gate logic. Present in the lock, `game-rules`, the SDK, the paymaster, the
paymaster-service, and wasm.

**In-VM tests** (the
[`tests/`](https://github.com/Kagwep/ckb-controller/tree/main/tests/src) crate,
`cargo test -p tests`) — run inside **CKB-VM** via `ckb-testtool` against the
actual `build/release/controller-session-lock`,
`build/release/controller-game-cell`, and the vendored `deps/auth`. They exercise
the real syscall paths (args, witness, header dep, `spawn_cell` → ckb-auth) that
host tests cannot. **Prerequisites:** `./build.sh` produced the binaries,
`deps/auth` is present, and a host `gcc` is on PATH (ckb-vm needs it — see
[invariants.md §11](./invariants.md)). Last verified run: **48/48 passing**
([README.md](https://github.com/Kagwep/ckb-controller/blob/main/README.md),
[plan.md](https://github.com/Kagwep/ckb-controller/blob/main/plan.md),
2026-07-08).

The rest of this page maps guarantees to tests, grouped by concern.

## Lock — the L1 session lock

Host unit tests in
[`contracts/controller-session-lock/src/main.rs`](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-session-lock/src/main.rs)
(`merkle_tests`, `session_tests`); in-VM tests in
[`tests/src/tests.rs`](https://github.com/Kagwep/ckb-controller/blob/main/tests/src/tests.rs).
Every in-VM test builds a real tx, signs it with a `ckb-testtool` secp key, and
runs it through the lock binary.

### Owner mode

| test | harness | guarantee |
|---|---|---|
| `owner_mode_unlocks` | in-VM | a valid owner signature unlocks full control. |
| `wrong_owner_signature_fails` | in-VM | a wrong key is refused by ckb-auth (`AuthError`). |
| `session_tests::parses_params` | host | the 96-byte params block parses at the right offsets. |

### Session models (registered & carried) + auth message

| test | harness | guarantee |
|---|---|---|
| `wildcard_session_unlocks` | in-VM | a registered session with a wildcard policy unlocks with no proofs. |
| `carried_model_unlocks_with_owner_authorization` | in-VM | carried model: params + owner authorization ride in the witness and re-verify on-chain. |
| `carried_model_rejects_unblessed_params` | in-VM | witness params the owner never signed → `AuthError` (cannot widen the cap). |
| `session_tests::registered_args_tail_equals_params_block` | host | registered args[20..116] parse identically to a standalone params block. |
| `session_tests::auth_message_deterministic_and_bound` | host | the carried auth message is deterministic and bound to params, script hash, and epoch. |

### Policy (TYPE and LOCK dimensions) + Merkle

| test | harness | guarantee |
|---|---|---|
| `scoped_session_policy_unlocks` | in-VM | a TYPE-scoped session unlocks when the output's type hash is in the root (with a Merkle proof). |
| `scoped_session_rejects_disallowed_type` | in-VM | an output whose type isn't in the root → `PolicyNotAllowed`. |
| `channel_session_funds_channel` | in-VM | a LOCK-scoped session funds only the allowlisted funding-lock (single-leaf, empty proof). |
| `channel_session_rejects_unlisted_destination` | in-VM | value to any other lock → `PolicyNotAllowed`, even within the cap. |
| `merkle_tests::hash_pair_is_order_independent` | host | sorted-pair node hashing is order-independent. |
| `merkle_tests::verifies_valid_proof` / `rejects_corrupted_proof` | host | membership verify accepts a good proof, rejects a corrupted one. |
| `merkle_tests::single_leaf_root_is_leaf` / `rejects_misaligned_proof` | host | single-leaf root == leaf; a non-32-multiple proof is rejected. |

### Expiry

| test | harness | guarantee |
|---|---|---|
| `expired_session_fails` | in-VM | a header timestamp past `expires_at` → `SessionExpired`. |
| `no_expiry_session_unlocks_without_header_dep` | in-VM | `NO_EXPIRY` skips the header read, so a no-header-dep tx (Fiber funding) unlocks — see [invariants.md §4](./invariants.md). |

### Spend cap

| test | harness | guarantee |
|---|---|---|
| `session_within_spend_cap_unlocks` | in-VM | net outflow ≤ cap passes. |
| `session_rejected_over_spend_cap` | in-VM | net outflow > cap → `SpendCapExceeded`. |

### Guardian co-signer

| test | harness | guarantee |
|---|---|---|
| `guardian_required_unlocks` | in-VM | session + correct guardian co-sign passes. |
| `guardian_missing_signature_fails` | in-VM | a configured guardian with no co-sig → `WitnessLenError`. |
| `guardian_wrong_signature_fails` | in-VM | a wrong guardian co-sig → `AuthError`. |

### Revocation + account-data continuity

| test | harness | guarantee |
|---|---|---|
| `carried_session_revoked_by_epoch_bump` | in-VM | an authorization signed for a stale epoch fails once the account advances. |
| `carried_session_rebless_after_revocation_unlocks` | in-VM | re-authorizing for the current epoch works again. |
| `session_cannot_alter_account_data` | in-VM | a session recreating the account with changed data → `SessionCannotAdminister` (cannot reset the epoch). |
| `session_tests::auth_message_deterministic_and_bound` | host | changing the epoch changes the auth message (the mechanism behind revocation). |

## Game type script + shared rules

The rules live in
[`game-rules/src/lib.rs`](https://github.com/Kagwep/ckb-controller/blob/main/game-rules/src/lib.rs)
(host unit tests) and are re-derived on-chain by
[`contracts/controller-game-cell/src/main.rs`](https://github.com/Kagwep/ckb-controller/blob/main/contracts/controller-game-cell/src/main.rs);
in-VM coverage is
[`tests/src/game_tests.rs`](https://github.com/Kagwep/ckb-controller/blob/main/tests/src/game_tests.rs).

### Rules (host unit — the transition function itself)

| test | guarantee |
|---|---|
| `state_roundtrip` / `empty_and_zero_length_agree` | encode/decode round-trips; empty data ≡ empty state. |
| `apply_batch_bumps_seq_and_accumulates` | a batch accumulates scores and bumps `seq` by the count applied. |
| `first_move_must_be_nonce_one` / `rejects_stale_nonce` | nonce discipline: first move is nonce 1, replays are rejected. |
| `points_over_max_rejected` | the per-move rule cap (`MAX_POINTS_PER_MOVE`). |
| `batch_roundtrip` | batch framing round-trips. |
| `equals_is_order_insensitive` | the comparison the type script uses ignores player order but not scores/seq. |

### Genesis / transition / replay / tamper (in-VM — the enforced version)

| test | guarantee |
|---|---|
| `genesis_creates_empty_game` / `genesis_accepts_zero_length_data` | genesis `(0 in,1 out)` from an empty (or zero-length) state passes. |
| `genesis_with_prefilled_state_rejected` / `genesis_with_nonzero_seq_rejected` | a genesis that mints pre-credited scores or a nonzero seq → `GenesisNotEmpty`. |
| `single_move_from_empty_applies` / `batch_two_players_applies` / `existing_player_accumulates` | signed transitions apply correctly. |
| `forged_signature_rejected` / `wrong_game_id_in_signature_rejected` | ckb-auth rejects a forged intent sig and an intent signed for another game. |
| `replayed_nonce_rejected` | a stale-nonce intent → `NonceNotFresh`. |
| `tampered_output_score_rejected` / `tampered_output_seq_rejected` | operator tampering of the output state/seq → `StateMismatch`. |
| `points_over_max_rejected` | the rule cap, enforced on-chain (`PointsTooHigh`). |
| `two_output_game_cells_rejected` | splitting the cell → `BadTxShape`. |
| `empty_batch_is_a_noop_transition` | an empty batch is a valid no-op (the mechanism behind `game grow` — [invariants.md §6](./invariants.md)). |

## SDK / paymaster / service / operator — drift guards

These run the OFF-CHAIN builders and then verify their output against the real
binaries in CKB-VM. A failure means an off-chain encoder drifted from the lock or
type script.

### SDK ↔ lock ([`tests/src/sdk_sanity.rs`](https://github.com/Kagwep/ckb-controller/blob/main/tests/src/sdk_sanity.rs), in-VM)

| test | guarantee |
|---|---|
| `sdk_owner_mode` | an SDK-built owner tx passes the real lock. |
| `sdk_scoped_session` | SDK args + params + Merkle root/proof + witness pass a scoped session. |
| `sdk_channel_session_funding_tx` | the `ChannelSession` dev API builds a funding tx the real lock accepts, and the off-chain `pay` loop settles arithmetic exactly. |
| `sdk_carried_session` | SDK-built carried-model witness + auth message pass. |

### Paymaster gate + assemble-then-balance

Host unit tests in
[`paymaster/src/`](https://github.com/Kagwep/ckb-controller/tree/main/paymaster/src)
(`authz.rs`, `biscuit_gate.rs`, `assemble.rs`); in-VM end-to-end in
[`tests/src/paymaster_sanity.rs`](https://github.com/Kagwep/ckb-controller/blob/main/tests/src/paymaster_sanity.rs).

| test | harness | guarantee |
|---|---|---|
| `authz::{valid_token_authorizes, expired_token_rejected, wrong_scope_rejected, wrong_authority_rejected, tampered_token_rejected}` | host | the Ed25519 reference gate accepts a valid capability and rejects expiry/scope/authority/tamper. |
| `biscuit_gate::{valid_token_authorizes, expired_token_denied, wrong_scope_denied, wrong_authority_rejected, tampered_token_rejected, revoked_token_denied, attenuated_token_honours_added_caveat}` | host | the production `biscuit-auth` gate — same contract, plus offline attenuation + revocation-by-id. |
| `assemble::{balance_appends_fee_input_and_change, insufficient_fee_cell_rejected}` | host | balancing appends the fee input + change; too-small a fee cell is refused. |
| `paymaster_sponsors_session_tx` | in-VM | full sponsored relay: gate → assemble-then-balance → client signs LAST → real lock accepts. |
| `gameplay_session_loop` | in-VM | one owner approval holds a 5-action session, each sponsored + session-signed within the per-tx cap. |
| `biscuit_gate_sponsors_session_tx` | in-VM | the biscuit gate sponsors a session tx the real lock accepts (gate is swappable end-to-end). |
| `paymaster_refuses_unauthorized` / `biscuit_gate_refuses_unauthorized` | in-VM crate | an expired token is refused before any tx work. |

### Service orchestration ([`tests/src/service_sanity.rs`](https://github.com/Kagwep/ckb-controller/blob/main/tests/src/service_sanity.rs))

| test | guarantee |
|---|---|
| `service_sponsors_session_tx_against_real_lock` | `SponsorService` over a mock node (gate → collect fee cell → balance) yields a lock-valid session tx. |
| `service_refuses_expired_token` | the service rejects an expired token (`ServiceError::Unauthorized`). |

Plus host unit tests in
[`paymaster-service/src/`](https://github.com/Kagwep/ckb-controller/tree/main/paymaster-service/src):
`lib.rs` (`sponsor_balances_authorized_request`, `sponsor_rejects_expired_token`,
`sponsor_reports_no_fee_cell`, `broadcast_forwards_to_node`) and `sighash.rs`
(`signature_recovers_to_the_signer_and_preserves_batch` — the standard
secp256k1_blake160 finalize must preserve the game cell's `input_type` batch).

### Operator queue behavior + operator ↔ type script

Host unit tests in
[`paymaster-service/src/operator.rs`](https://github.com/Kagwep/ckb-controller/blob/main/paymaster-service/src/operator.rs);
in-VM drift guard in
[`tests/src/game_operator_sanity.rs`](https://github.com/Kagwep/ckb-controller/blob/main/tests/src/game_operator_sanity.rs).

| test | harness | guarantee |
|---|---|---|
| `build_transition_applies_and_bumps_seq` | host | a transition applies the mempool and doesn't drain it (retryable). |
| `empty_mempool_cannot_flush` | host | nothing to flush → `Empty`, no broadcast. |
| `stale_intent_rejected_at_submit` / `bad_intent_does_not_poison_the_queue` | host | a doomed intent is refused at `submit`, never blocking the queue. |
| `flush_broadcasts_and_advances_tip` | host | flush broadcasts, advances the tip to `(hash, 0)`, drains the mempool. |
| `fee_shrinks_the_game_cell_each_transition` / `fee_larger_than_capacity_is_an_error` | host | self-funding shrinks the game cell per tx; an over-cap fee errors before broadcast. |
| `failed_finalize_does_not_advance` | host | a failed finalize leaves the tip + mempool untouched (retryable). |
| `operator_transition_verifies_from_empty` / `operator_transition_verifies_accumulate` | in-VM | operator-built transitions pass the REAL type script. |
| `operator_transition_with_forged_sig_rejected_on_chain` | in-VM | a forged intent the operator can't detect is rejected on-chain — safety doesn't depend on the operator. |

## wasm ↔ SDK byte equality

Host unit tests in
[`wasm/src/lib.rs`](https://github.com/Kagwep/ckb-controller/blob/main/wasm/src/lib.rs)
match the wasm surface against the SDK byte-for-byte, so the browser and the Rust
builder cannot diverge.

| test | guarantee |
|---|---|
| `args_roundtrip_matches_sdk` | wasm `session_params`/`registered_args` == the SDK's bytes. |
| `bad_hex_is_rejected` | malformed hex / wrong-length inputs throw instead of producing garbage. |
| `address_is_bech32m` | `controller_address` produces the RFC21 `ckt`/`ckb` bech32m address. |
| `tx_message_matches_sdk` | the cell_deps-cleared message from molecule hex == the SDK's. |
| `merkle_root_proof_verify` | wasm root/proof/verify agree with the SDK. |
| `full_channel_loop_in_wasm_surface` / `pay_over_budget_errors` | the open→pay→close loop and its budget guard behave like the SDK. |
| `game_intent_message_matches_sdk` / `game_encode_intent_is_101_bytes` | intent message + 101-byte encoding == the SDK / `game-rules`. |
| `game_apply_and_batch_match_sdk` / `game_decode_state_renders_board` | `game_apply` and the rendered board match the SDK transition. |

## Adding a guarantee

**Test it at the lowest harness that can catch it, plus one in-VM case if the
lock or type script enforces it.** Pure encoding/math → a host unit test in the
crate that owns it (fastest, and it pins the byte layout). A new constraint or
transition rule the chain enforces → also add an in-VM pass **and** reject case
in [`tests/`](https://github.com/Kagwep/ckb-controller/tree/main/tests/src),
because only CKB-VM exercises `spawn_cell` → ckb-auth and the real syscalls. If
you touched a byte layout, the wasm↔SDK and `*_sanity` drift guards are what prove
every implementation still agrees — keep them in the same PR
([wire-formats.md](./wire-formats.md)).
