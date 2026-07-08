//! End-to-end tests for the controller session lock, run in CKB-VM via
//! ckb-testtool. These exercise the real syscall paths (args, witness, header
//! dep, spawn_cell → ckb-auth) that the in-crate unit tests cannot.
//!
//! Prereqs: `./build.sh` (or `make build`) produced ../build/release/…, and the
//! ckb-auth binary is at ../deps/auth.

use ckb_testtool::builtin::ALWAYS_SUCCESS;
use ckb_testtool::ckb_crypto::secp::{Generator, Privkey};
use ckb_testtool::ckb_hash::blake2b_256;
use ckb_testtool::ckb_types::{
    bytes::Bytes,
    core::{HeaderBuilder, TransactionBuilder, TransactionView},
    packed::*,
    prelude::*,
};
use ckb_testtool::context::Context;
use std::fs;

const MAX_CYCLES: u64 = 100_000_000;

const MODE_OWNER: u8 = 0;
const MODE_SESSION: u8 = 1;
const WILDCARD_ROOT: [u8; 32] = [0xFFu8; 32];

fn lock_binary() -> Bytes {
    fs::read("../build/release/controller-session-lock")
        .expect("build the lock first: `./build.sh` or `make build`")
        .into()
}

fn auth_binary() -> Bytes {
    fs::read("../deps/auth")
        .expect("place the ckb-auth binary at deps/auth")
        .into()
}

/// Same commitment the lock uses: blake2b_256 of the raw tx with cell_deps
/// cleared (so a paymaster may attach fee cell_deps without breaking the sig).
fn compute_tx_message(tx: &TransactionView) -> [u8; 32] {
    let raw = tx
        .data()
        .raw()
        .as_builder()
        .cell_deps(Default::default())
        .build();
    blake2b_256(raw.as_slice())
}

fn pubkey_hash(privkey: &Privkey) -> [u8; 20] {
    let pubkey = privkey.pubkey().expect("pubkey");
    let h = blake2b_256(pubkey.serialize());
    h[0..20].try_into().unwrap()
}

fn sign(privkey: &Privkey, message: &[u8; 32]) -> Vec<u8> {
    privkey
        .sign_recoverable(&(*message).into())
        .expect("sign")
        .serialize()
}

/// args for the authorization-carried model: owner_pubkey_hash only (20 bytes).
fn owner_only_args(owner_hash: &[u8; 20]) -> Bytes {
    owner_hash.to_vec().into()
}

/// The 96-byte session-params block:
/// session_hash ‖ expires_at ‖ root ‖ spend_cap ‖ guardian_hash.
fn session_params_g(
    session_hash: &[u8; 20],
    expires_at: u64,
    policies_root: &[u8; 32],
    spend_cap: u128,
    guardian_hash: &[u8; 20],
) -> Vec<u8> {
    let mut p = Vec::with_capacity(96);
    p.extend_from_slice(session_hash);
    p.extend_from_slice(&expires_at.to_le_bytes());
    p.extend_from_slice(policies_root);
    p.extend_from_slice(&spend_cap.to_le_bytes());
    p.extend_from_slice(guardian_hash);
    p
}

/// Session params with no guardian (the common case).
fn session_params(
    session_hash: &[u8; 20],
    expires_at: u64,
    policies_root: &[u8; 32],
    spend_cap: u128,
) -> Vec<u8> {
    session_params_g(session_hash, expires_at, policies_root, spend_cap, &[0u8; 20])
}

/// args for the registered model (96 bytes): owner ‖ session params.
fn registered_args(
    owner_hash: &[u8; 20],
    session_hash: &[u8; 20],
    expires_at: u64,
    policies_root: &[u8; 32],
    spend_cap: u128,
) -> Bytes {
    let mut a = owner_hash.to_vec();
    a.extend_from_slice(&session_params(session_hash, expires_at, policies_root, spend_cap));
    a.into()
}

/// The owner-authorization message for the carried model — must match the lock's
/// `session_auth_message`: blake2b_256(domain ‖ script_hash ‖ params).
const SESSION_AUTH_DOMAIN: &[u8] = b"ckb-controller/session-auth/v1";
fn session_auth_message(script_hash: &[u8; 32], revocation_epoch: u64, params: &[u8]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(SESSION_AUTH_DOMAIN.len() + 32 + 8 + params.len());
    buf.extend_from_slice(SESSION_AUTH_DOMAIN);
    buf.extend_from_slice(script_hash);
    buf.extend_from_slice(&revocation_epoch.to_le_bytes());
    buf.extend_from_slice(params);
    blake2b_256(buf)
}

/// Account cell data encoding the revocation epoch (empty for epoch 0).
fn epoch_data(epoch: u64) -> Bytes {
    if epoch == 0 {
        Bytes::new()
    } else {
        epoch.to_le_bytes().to_vec().into()
    }
}

fn script_hash_of(script: &Script) -> [u8; 32] {
    script.calc_script_hash().as_slice().try_into().unwrap()
}

fn witness_with_lock(lock_bytes: Vec<u8>) -> Bytes {
    WitnessArgs::new_builder()
        .lock(Some(Bytes::from(lock_bytes)).pack())
        .build()
        .as_bytes()
}

// --- Merkle helpers, mirroring the lock's scheme exactly --------------------
// leaf = blake2b_256(type_script_hash); node = blake2b_256(min(a,b) ‖ max(a,b)).

fn policy_leaf(type_script: &Script) -> [u8; 32] {
    blake2b_256(type_script.calc_script_hash().as_slice())
}

fn hash_pair(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut buf = [0u8; 64];
    if a <= b {
        buf[0..32].copy_from_slice(a);
        buf[32..64].copy_from_slice(b);
    } else {
        buf[0..32].copy_from_slice(b);
        buf[32..64].copy_from_slice(a);
    }
    blake2b_256(buf)
}

#[test]
fn owner_mode_unlocks() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());

    let owner = Generator::new().gen_privkey();
    let args = owner_only_args(&pubkey_hash(&owner));
    let lock_script = context.build_script(&lock_op, args).expect("script");

    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
    ]
    .pack();

    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );
    let input = CellInput::new_builder().previous_output(input_op).build();
    let recipient = Script::new_builder()
        .args(Bytes::from("recipient").pack())
        .build();
    let outputs = vec![CellOutput::new_builder()
        .capacity(1000u64.pack())
        .lock(recipient)
        .build()];

    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .input(input)
        .outputs(outputs)
        .outputs_data(vec![Bytes::new()].pack())
        .build();

    let message = compute_tx_message(&tx);
    let mut lock_bytes = vec![MODE_OWNER];
    lock_bytes.extend_from_slice(&sign(&owner, &message));
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("owner mode should pass");
    println!("owner_mode_unlocks cycles: {cycles}");
}

#[test]
fn wrong_owner_signature_fails() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());

    let owner = Generator::new().gen_privkey();
    let attacker = Generator::new().gen_privkey();
    let args = owner_only_args(&pubkey_hash(&owner));
    let lock_script = context.build_script(&lock_op, args).expect("script");

    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
    ]
    .pack();
    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        Bytes::new(),
    );
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(Script::new_builder().build())
            .build()])
        .outputs_data(vec![Bytes::new()].pack())
        .build();

    let message = compute_tx_message(&tx);
    let mut lock_bytes = vec![MODE_OWNER];
    lock_bytes.extend_from_slice(&sign(&attacker, &message)); // wrong key
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("wrong owner signature must be rejected (AuthError)");
}

/// Helper: build a single-input session tx with a header dep at `header_ts_ms`,
/// returning the unsigned tx + the session privkey embedded in `args`.
fn session_tx(
    context: &mut Context,
    lock_op: &OutPoint,
    auth_op: &OutPoint,
    args: Bytes,
    header_ts_ms: u64,
) -> (TransactionView, Byte32) {
    let lock_script = context.build_script(lock_op, args).expect("script");

    let header = HeaderBuilder::default()
        .timestamp(header_ts_ms.pack())
        .build();
    context.insert_header(header.clone());

    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op.clone()).build(),
        CellDep::new_builder().out_point(auth_op.clone()).build(),
    ]
    .pack();

    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        Bytes::new(),
    );
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(Script::new_builder().args(Bytes::from("recipient").pack()).build())
            .build()])
        .outputs_data(vec![Bytes::new()].pack())
        .build();
    (tx, header.hash())
}

#[test]
fn wildcard_session_unlocks() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());

    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    // wildcard policy, unlimited spend cap, far-future expiry.
    let expires_at = 2_000_000_000u64; // ~2033
    let args = registered_args(
        &pubkey_hash(&owner),
        &pubkey_hash(&session),
        expires_at,
        &WILDCARD_ROOT,
        u128::MAX,
    );
    // header timestamp well before expiry.
    let (tx, _h) = session_tx(&mut context, &lock_op, &auth_op, args, 1_700_000_000_000);

    let message = compute_tx_message(&tx);
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&sign(&session, &message)); // wildcard => no proofs
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("wildcard session should pass");
    println!("wildcard_session_unlocks cycles: {cycles}");
}

/// A NO_EXPIRY session (expires_at == u64::MAX) must unlock a transaction that
/// carries NO header dep — the lock skips its expiry/header read. This is what
/// lets a controller session sign a counterparty-built tx (e.g. a Fiber funding
/// tx), which has no header dep. Proven end-to-end on a live Fiber channel.
#[test]
fn no_expiry_session_unlocks_without_header_dep() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());

    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    // NO_EXPIRY sentinel; wildcard policy; unlimited spend cap.
    let args = registered_args(
        &pubkey_hash(&owner),
        &pubkey_hash(&session),
        u64::MAX,
        &WILDCARD_ROOT,
        u128::MAX,
    );
    let lock_script = context.build_script(&lock_op, args).expect("script");

    // NO header dep, and no header inserted — the lock must not read one.
    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
    ]
    .pack();
    let input_op = context.create_cell(
        CellOutput::new_builder().capacity(1000u64.pack()).lock(lock_script).build(),
        Bytes::new(),
    );
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(Script::new_builder().args(Bytes::from("recipient").pack()).build())
            .build()])
        .outputs_data(vec![Bytes::new()].pack())
        .build();

    let message = compute_tx_message(&tx);
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&sign(&session, &message));
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("NO_EXPIRY session should unlock without a header dep");
    println!("no_expiry_session_unlocks_without_header_dep cycles: {cycles}");
}

#[test]
fn expired_session_fails() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());

    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    let expires_at = 1_600_000_000u64; // in the past relative to the header below
    let args = registered_args(
        &pubkey_hash(&owner),
        &pubkey_hash(&session),
        expires_at,
        &WILDCARD_ROOT,
        u128::MAX,
    );
    // header timestamp AFTER expiry -> SessionExpired.
    let (tx, _h) = session_tx(&mut context, &lock_op, &auth_op, args, 1_700_000_000_000);

    let message = compute_tx_message(&tx);
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&sign(&session, &message));
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("expired session must be rejected (SessionExpired)");
}

/// A scoped (non-wildcard) session: the allowed-policies root commits to a set
/// of type-script hashes, and the witness carries a Merkle proof for each typed
/// output. Here the allowed set is {type_a, type_b}; the output uses type_a and
/// the witness proves membership, so it unlocks.
#[test]
fn scoped_session_policy_unlocks() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let as_op = context.deploy_cell(ALWAYS_SUCCESS.clone());

    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    // Two allowed type scripts (always-success code so the output's type script
    // executes successfully), distinguished by args.
    let type_a = context
        .build_script(&as_op, Bytes::from("type-A"))
        .expect("type a");
    let type_b = context
        .build_script(&as_op, Bytes::from("type-B"))
        .expect("type b");
    let leaf_a = policy_leaf(&type_a);
    let leaf_b = policy_leaf(&type_b);
    let root = hash_pair(&leaf_a, &leaf_b);

    let args = registered_args(
        &pubkey_hash(&owner),
        &pubkey_hash(&session),
        2_000_000_000u64,
        &root,
        u128::MAX,
    );
    let lock_script = context.build_script(&lock_op, args).expect("script");

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());

    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
        CellDep::new_builder().out_point(as_op).build(),
    ]
    .pack();

    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        Bytes::new(),
    );
    // A game-state output carrying an allowed type script.
    let outputs = vec![CellOutput::new_builder()
        .capacity(1000u64.pack())
        .lock(Script::new_builder().args(Bytes::from("game-state").pack()).build())
        .type_(Some(type_a).pack())
        .build()];

    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(outputs)
        .outputs_data(vec![Bytes::new()].pack())
        .build();

    let message = compute_tx_message(&tx);
    // witness: mode ‖ session_sig ‖ [kind=TYPE(0) ‖ proof_len=1 ‖ sibling=leaf_b].
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&sign(&session, &message));
    lock_bytes.push(0u8); // POLICY_KIND_TYPE
    lock_bytes.push(1u8); // proof_len
    lock_bytes.extend_from_slice(&leaf_b);
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("scoped session within policy should pass");
    println!("scoped_session_policy_unlocks cycles: {cycles}");
}

/// Same scoped session, but the output uses a type script NOT in the allowed
/// root. No proof can satisfy the root, so the lock rejects with PolicyNotAllowed.
#[test]
fn scoped_session_rejects_disallowed_type() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let as_op = context.deploy_cell(ALWAYS_SUCCESS.clone());

    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    let type_a = context
        .build_script(&as_op, Bytes::from("type-A"))
        .expect("type a");
    let type_b = context
        .build_script(&as_op, Bytes::from("type-B"))
        .expect("type b");
    let type_c = context
        .build_script(&as_op, Bytes::from("type-C")) // NOT in the root
        .expect("type c");
    let root = hash_pair(&policy_leaf(&type_a), &policy_leaf(&type_b));

    let args = registered_args(
        &pubkey_hash(&owner),
        &pubkey_hash(&session),
        2_000_000_000u64,
        &root,
        u128::MAX,
    );
    let lock_script = context.build_script(&lock_op, args).expect("script");

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());

    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
        CellDep::new_builder().out_point(as_op).build(),
    ]
    .pack();

    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        Bytes::new(),
    );
    let outputs = vec![CellOutput::new_builder()
        .capacity(1000u64.pack())
        .lock(Script::new_builder().build())
        .type_(Some(type_c).pack()) // disallowed
        .build()];

    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(outputs)
        .outputs_data(vec![Bytes::new()].pack())
        .build();

    let message = compute_tx_message(&tx);
    // supply a (wrong) type proof; type_c's leaf can't hash to the root.
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&sign(&session, &message));
    lock_bytes.push(0u8); // POLICY_KIND_TYPE
    lock_bytes.push(1u8); // proof_len
    lock_bytes.extend_from_slice(&policy_leaf(&type_b));
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("disallowed output type must be rejected (PolicyNotAllowed)");
}

/// A *channel-funding* session: scoped (by LOCK policy) to only move value into
/// the Fiber funding-lock (here a stand-in) or back to the account, with the
/// spend cap as the channel budget. Funds a channel cell + recreates the account.
#[test]
fn channel_session_funds_channel() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    // The one allowed channel destination (stand-in for Fiber's funding-lock).
    let funding_lock = Script::new_builder()
        .code_hash([0xABu8; 32].pack())
        .args(Bytes::from("fiber-funding-lock").pack())
        .build();
    let root = policy_leaf(&funding_lock); // single-leaf tree => root == leaf

    // session scoped to that lock; spend cap 500 = channel budget.
    let args = registered_args(&pubkey_hash(&owner), &pubkey_hash(&session), 2_000_000_000, &root, 500);
    let lock_script = context.build_script(&lock_op, args).expect("script");

    let header = HeaderBuilder::default().timestamp(1_700_000_000_000u64.pack()).build();
    context.insert_header(header.clone());
    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
    ]
    .pack();
    let input_op = context.create_cell(
        CellOutput::new_builder().capacity(1000u64.pack()).lock(lock_script.clone()).build(),
        Bytes::new(),
    );
    // outputs: [account change (self, 600) , channel funding cell (400)]
    let outputs = vec![
        CellOutput::new_builder().capacity(600u64.pack()).lock(lock_script.clone()).build(),
        CellOutput::new_builder().capacity(400u64.pack()).lock(funding_lock).build(),
    ];
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(outputs)
        .outputs_data(vec![Bytes::new(), Bytes::new()].pack())
        .build();

    let message = compute_tx_message(&tx);
    // witness: mode ‖ session_sig ‖ [kind=LOCK(1) ‖ proof_len=0] for the funding cell.
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&sign(&session, &message));
    lock_bytes.push(1u8); // POLICY_KIND_LOCK
    lock_bytes.push(0u8); // proof_len = 0 (single-leaf root)
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("session funding the allowed channel lock should pass");
    println!("channel_session_funds_channel cycles: {cycles}");
}

/// The channel session may NOT send value to any lock other than the allowed
/// funding-lock (or the account itself) — even within the spend cap.
#[test]
fn channel_session_rejects_unlisted_destination() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    let funding_lock = Script::new_builder()
        .code_hash([0xABu8; 32].pack())
        .args(Bytes::from("fiber-funding-lock").pack())
        .build();
    let root = policy_leaf(&funding_lock);
    let args = registered_args(&pubkey_hash(&owner), &pubkey_hash(&session), 2_000_000_000, &root, 500);
    let lock_script = context.build_script(&lock_op, args).expect("script");

    let header = HeaderBuilder::default().timestamp(1_700_000_000_000u64.pack()).build();
    context.insert_header(header.clone());
    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
    ]
    .pack();
    let input_op = context.create_cell(
        CellOutput::new_builder().capacity(1000u64.pack()).lock(lock_script.clone()).build(),
        Bytes::new(),
    );
    // attacker destination, NOT the allowlisted funding-lock
    let other_lock = Script::new_builder()
        .code_hash([0xCDu8; 32].pack())
        .args(Bytes::from("attacker").pack())
        .build();
    let outputs = vec![
        CellOutput::new_builder().capacity(600u64.pack()).lock(lock_script.clone()).build(),
        CellOutput::new_builder().capacity(400u64.pack()).lock(other_lock).build(),
    ];
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(outputs)
        .outputs_data(vec![Bytes::new(), Bytes::new()].pack())
        .build();

    let message = compute_tx_message(&tx);
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&sign(&session, &message));
    lock_bytes.push(1u8); // POLICY_KIND_LOCK
    lock_bytes.push(0u8);
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("funding a non-allowlisted destination must be rejected (PolicyNotAllowed)");
}

/// Authorization-carried model: the account args hold ONLY the owner hash. The
/// session params + an owner signature blessing them ride in the witness and are
/// re-verified on-chain — no on-chain session registration.
#[test]
fn carried_model_unlocks_with_owner_authorization() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());

    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    // 20-byte (owner-only) args select the carried model.
    let lock_script = context
        .build_script(&lock_op, owner_only_args(&pubkey_hash(&owner)))
        .expect("script");
    let script_hash = script_hash_of(&lock_script);

    // Owner blesses these session params off-chain (wildcard, unlimited).
    let params = session_params(&pubkey_hash(&session), 2_000_000_000, &WILDCARD_ROOT, u128::MAX);
    // account cell data is empty => revocation epoch 0.
    let owner_auth = sign(&owner, &session_auth_message(&script_hash, 0, &params));

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());

    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
    ]
    .pack();
    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        Bytes::new(),
    );
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(Script::new_builder().args(Bytes::from("recipient").pack()).build())
            .build()])
        .outputs_data(vec![Bytes::new()].pack())
        .build();

    let session_sig = sign(&session, &compute_tx_message(&tx));

    // witness lock = mode ‖ params(76) ‖ owner_auth(65) ‖ session_sig(65)
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&params);
    lock_bytes.extend_from_slice(&owner_auth);
    lock_bytes.extend_from_slice(&session_sig);
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("carried-model session should pass");
    println!("carried_model_unlocks cycles: {cycles}");
}

/// Carried model, but the witness carries params the owner never signed (a
/// widened spend cap). The owner authorization no longer matches → AuthError.
#[test]
fn carried_model_rejects_unblessed_params() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());

    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();
    let lock_script = context
        .build_script(&lock_op, owner_only_args(&pubkey_hash(&owner)))
        .expect("script");
    let script_hash = script_hash_of(&lock_script);

    // Owner blesses params with a *limited* cap...
    let blessed = session_params(&pubkey_hash(&session), 2_000_000_000, &WILDCARD_ROOT, 100);
    let owner_auth = sign(&owner, &session_auth_message(&script_hash, 0, &blessed));
    // ...but the attacker puts *unlimited* cap in the witness.
    let forged = session_params(&pubkey_hash(&session), 2_000_000_000, &WILDCARD_ROOT, u128::MAX);

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());
    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
    ]
    .pack();
    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        Bytes::new(),
    );
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(Script::new_builder().build())
            .build()])
        .outputs_data(vec![Bytes::new()].pack())
        .build();

    let session_sig = sign(&session, &compute_tx_message(&tx));
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&forged); // mismatch vs what owner signed
    lock_bytes.extend_from_slice(&owner_auth);
    lock_bytes.extend_from_slice(&session_sig);
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("unblessed (forged) session params must be rejected (AuthError)");
}

/// Build a registered-model session tx that moves value out of the account:
/// input = `input_cap`, output[0] returns `return_cap` to the account lock, the
/// remainder goes to a recipient. Net outflow = input_cap - return_cap.
fn spend_cap_tx(
    context: &mut Context,
    lock_op: &OutPoint,
    auth_op: &OutPoint,
    owner: &Privkey,
    session: &Privkey,
    spend_cap: u128,
    input_cap: u64,
    return_cap: u64,
) -> TransactionView {
    let args = registered_args(
        &pubkey_hash(owner),
        &pubkey_hash(session),
        2_000_000_000,
        &WILDCARD_ROOT,
        spend_cap,
    );
    let lock_script = context.build_script(lock_op, args).expect("script");

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());
    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op.clone()).build(),
        CellDep::new_builder().out_point(auth_op.clone()).build(),
    ]
    .pack();
    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(input_cap.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![
            // output[0]: capacity that stays under the account lock (must keep args).
            CellOutput::new_builder()
                .capacity(return_cap.pack())
                .lock(lock_script)
                .build(),
            // output[1]: the rest leaves the account to a recipient.
            CellOutput::new_builder()
                .capacity((input_cap - return_cap).pack())
                .lock(Script::new_builder().args(Bytes::from("recipient").pack()).build())
                .build(),
        ])
        .outputs_data(vec![Bytes::new(); 2].pack())
        .build();

    let session_sig = sign(session, &compute_tx_message(&tx));
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&session_sig); // wildcard: no proofs
    tx.as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build()
}

#[test]
fn session_within_spend_cap_unlocks() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    // outflow = 1000 - 850 = 150 <= cap 200 -> passes.
    let tx = spend_cap_tx(&mut context, &lock_op, &auth_op, &owner, &session, 200, 1000, 850);
    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("within spend cap should pass");
    println!("session_within_spend_cap cycles: {cycles}");
}

#[test]
fn session_rejected_over_spend_cap() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    // outflow = 1000 - 850 = 150 > cap 100 -> SpendCapExceeded.
    let tx = spend_cap_tx(&mut context, &lock_op, &auth_op, &owner, &session, 100, 1000, 850);
    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("net outflow over spend cap must be rejected (SpendCapExceeded)");
}

/// Build a registered-model session whose params set a guardian co-signer.
/// `guardian_sig` selects which (if any) guardian signature goes in the witness.
enum Guardian {
    Correct,
    Wrong,
    Missing,
}

fn guardian_tx(
    context: &mut Context,
    lock_op: &OutPoint,
    auth_op: &OutPoint,
    owner: &Privkey,
    session: &Privkey,
    guardian: &Privkey,
    which: Guardian,
) -> TransactionView {
    let mut args = pubkey_hash(owner).to_vec();
    args.extend_from_slice(&session_params_g(
        &pubkey_hash(session),
        2_000_000_000,
        &WILDCARD_ROOT,
        u128::MAX,
        &pubkey_hash(guardian),
    ));
    let lock_script = context
        .build_script(lock_op, Bytes::from(args))
        .expect("script");

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());
    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op.clone()).build(),
        CellDep::new_builder().out_point(auth_op.clone()).build(),
    ]
    .pack();
    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        Bytes::new(),
    );
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(Script::new_builder().build())
            .build()])
        .outputs_data(vec![Bytes::new()].pack())
        .build();

    let message = compute_tx_message(&tx);
    // witness: mode ‖ session_sig ‖ [guardian_sig]
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&sign(session, &message));
    match which {
        Guardian::Correct => lock_bytes.extend_from_slice(&sign(guardian, &message)),
        Guardian::Wrong => {
            let impostor = Generator::new().gen_privkey();
            lock_bytes.extend_from_slice(&sign(&impostor, &message));
        }
        Guardian::Missing => {} // omit the guardian signature entirely
    }
    tx.as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build()
}

#[test]
fn guardian_required_unlocks() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let (owner, session, guardian) = (
        Generator::new().gen_privkey(),
        Generator::new().gen_privkey(),
        Generator::new().gen_privkey(),
    );
    let tx = guardian_tx(
        &mut context, &lock_op, &auth_op, &owner, &session, &guardian, Guardian::Correct,
    );
    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("session + correct guardian co-sign should pass");
    println!("guardian_required_unlocks cycles: {cycles}");
}

#[test]
fn guardian_missing_signature_fails() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let (owner, session, guardian) = (
        Generator::new().gen_privkey(),
        Generator::new().gen_privkey(),
        Generator::new().gen_privkey(),
    );
    let tx = guardian_tx(
        &mut context, &lock_op, &auth_op, &owner, &session, &guardian, Guardian::Missing,
    );
    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("missing guardian signature must be rejected (WitnessLenError)");
}

#[test]
fn guardian_wrong_signature_fails() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let (owner, session, guardian) = (
        Generator::new().gen_privkey(),
        Generator::new().gen_privkey(),
        Generator::new().gen_privkey(),
    );
    let tx = guardian_tx(
        &mut context, &lock_op, &auth_op, &owner, &session, &guardian, Guardian::Wrong,
    );
    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("wrong guardian signature must be rejected (AuthError)");
}

/// Build a carried-model session tx where the account cell's data encodes
/// `input_epoch` and the owner blessed the session for `signed_epoch`. They must
/// match for the owner authorization to verify — the revocation mechanism.
fn carried_tx(
    context: &mut Context,
    lock_op: &OutPoint,
    auth_op: &OutPoint,
    owner: &Privkey,
    session: &Privkey,
    input_epoch: u64,
    signed_epoch: u64,
) -> TransactionView {
    let lock_script = context
        .build_script(lock_op, owner_only_args(&pubkey_hash(owner)))
        .expect("script");
    let script_hash = script_hash_of(&lock_script);
    let params = session_params(&pubkey_hash(session), 2_000_000_000, &WILDCARD_ROOT, u128::MAX);
    let owner_auth = sign(owner, &session_auth_message(&script_hash, signed_epoch, &params));

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());
    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op.clone()).build(),
        CellDep::new_builder().out_point(auth_op.clone()).build(),
    ]
    .pack();
    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        epoch_data(input_epoch),
    );
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(Script::new_builder().build())
            .build()])
        .outputs_data(vec![Bytes::new()].pack())
        .build();

    let session_sig = sign(session, &compute_tx_message(&tx));
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&params);
    lock_bytes.extend_from_slice(&owner_auth);
    lock_bytes.extend_from_slice(&session_sig);
    tx.as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build()
}

#[test]
fn carried_session_revoked_by_epoch_bump() {
    // Owner blessed at epoch 0; the account has since advanced to epoch 1
    // (revoked). The stale authorization no longer verifies.
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();
    let tx = carried_tx(&mut context, &lock_op, &auth_op, &owner, &session, 1, 0);
    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("revoked (stale-epoch) carried session must fail (AuthError)");
}

#[test]
fn carried_session_rebless_after_revocation_unlocks() {
    // Owner re-blesses the session for the current epoch 1 -> works again.
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();
    let tx = carried_tx(&mut context, &lock_op, &auth_op, &owner, &session, 1, 1);
    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("re-blessed carried session should pass");
    println!("carried_rebless cycles: {cycles}");
}

/// A session may not reset the account's revocation epoch (in cell data) to evade
/// a revoke: recreating the account cell with changed data is rejected.
#[test]
fn session_cannot_alter_account_data() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    let args = registered_args(
        &pubkey_hash(&owner),
        &pubkey_hash(&session),
        2_000_000_000,
        &WILDCARD_ROOT,
        u128::MAX,
    );
    let lock_script = context.build_script(&lock_op, args).expect("script");
    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());
    let cell_deps = vec![
        CellDep::new_builder().out_point(lock_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
    ]
    .pack();
    // input account cell at epoch 0 (empty data).
    let input_op = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );
    // output recreates the account lock but bumps the epoch in data (evasion).
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build()])
        .outputs_data(vec![epoch_data(5)].pack())
        .build();

    let session_sig = sign(&session, &compute_tx_message(&tx));
    let mut lock_bytes = vec![MODE_SESSION];
    lock_bytes.extend_from_slice(&session_sig);
    let tx = tx
        .as_advanced_builder()
        .witness(witness_with_lock(lock_bytes).pack())
        .build();

    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("session altering account data must fail (SessionCannotAdminister)");
}
