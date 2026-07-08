//! Sanity tests: build transactions entirely with `controller-sdk` (args, params,
//! Merkle root + proofs, messages, witnesses) and verify them against the REAL
//! lock binary in CKB-VM. If the SDK and the lock ever drift, these fail.

use ckb_testtool::builtin::ALWAYS_SUCCESS;
use ckb_testtool::ckb_crypto::secp::{Generator, Privkey};
use ckb_testtool::ckb_hash::blake2b_256;
use ckb_testtool::ckb_types::{
    bytes::Bytes,
    core::{HeaderBuilder, TransactionBuilder},
    packed::*,
    prelude::*,
};
use ckb_testtool::context::Context;
use controller_sdk as sdk;
use std::fs;

const MAX_CYCLES: u64 = 100_000_000;
const FAR_FUTURE: u64 = 2_000_000_000;
const HEADER_TS_MS: u64 = 1_700_000_000_000; // ~1.7e9 s, before FAR_FUTURE

fn lock_binary() -> Bytes {
    fs::read("../build/release/controller-session-lock")
        .expect("build the lock first: ./build.sh")
        .into()
}
fn auth_binary() -> Bytes {
    fs::read("../deps/auth").expect("deps/auth").into()
}
fn pubkey_hash(privkey: &Privkey) -> [u8; 20] {
    blake2b_256(privkey.pubkey().expect("pubkey").serialize())[0..20]
        .try_into()
        .unwrap()
}
fn sign(privkey: &Privkey, message: &[u8; 32]) -> Vec<u8> {
    privkey
        .sign_recoverable(&(*message).into())
        .expect("sign")
        .serialize()
}
fn script_hash(script: &Script) -> [u8; 32] {
    script.calc_script_hash().as_slice().try_into().unwrap()
}

#[test]
fn sdk_owner_mode() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let owner = Generator::new().gen_privkey();

    let lock_script = context
        .build_script(&lock_op, sdk::owner_only_args(&pubkey_hash(&owner)))
        .expect("script");
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

    let sig = sign(&owner, &sdk::tx_message(&tx));
    let tx = tx
        .as_advanced_builder()
        .witness(sdk::owner_witness(&sig).pack())
        .build();

    context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("SDK-built owner tx should pass");
}

#[test]
fn sdk_scoped_session() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let as_op = context.deploy_cell(ALWAYS_SUCCESS.clone());
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    // Allowed policy set = three type scripts; the output uses the first.
    let type_a = context.build_script(&as_op, Bytes::from("A")).unwrap();
    let type_b = context.build_script(&as_op, Bytes::from("B")).unwrap();
    let type_c = context.build_script(&as_op, Bytes::from("C")).unwrap();
    let leaves = [
        sdk::policy_leaf_for_type_script(&type_a),
        sdk::policy_leaf_for_type_script(&type_b),
        sdk::policy_leaf_for_type_script(&type_c),
    ];
    let root = sdk::merkle_root(&leaves);
    let proof = sdk::merkle_proof(&leaves, 0); // proof for type_a

    let params = sdk::session_params(
        &pubkey_hash(&session),
        FAR_FUTURE,
        &root,
        sdk::SPEND_CAP_UNLIMITED,
        &[0u8; 20],
    );
    let args = sdk::registered_args(&pubkey_hash(&owner), &params);
    let lock_script = context.build_script(&lock_op, args).expect("script");

    let header = HeaderBuilder::default().timestamp(HEADER_TS_MS.pack()).build();
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
    let tx = TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(Script::new_builder().args(Bytes::from("game").pack()).build())
            .type_(Some(type_a).pack())
            .build()])
        .outputs_data(vec![Bytes::new()].pack())
        .build();

    let session_sig = sign(&session, &sdk::tx_message(&tx));
    let proof_region = sdk::proof_region(&[(sdk::POLICY_KIND_TYPE, proof)]);
    let witness = sdk::session_witness_registered(&session_sig, None, &proof_region);
    let tx = tx.as_advanced_builder().witness(witness.pack()).build();

    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("SDK-built scoped session tx should pass");
    println!("sdk_scoped_session cycles: {cycles}");
}

/// The `ChannelSession` dev API (roadmap step 2): build a real funding tx via
/// `open`, session-sign it, and prove the REAL lock accepts it — funding only the
/// allowlisted Fiber funding-lock, within the budget = spend cap. Also runs the
/// full off-chain `pay` loop on the `MockRail` and checks the net settlement.
#[test]
fn sdk_channel_session_funding_tx() {
    use sdk::channel::{ChannelConfig, ChannelSession, MockRail};

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

    // Session scoped to that lock, spend cap = budget (500).
    let params = sdk::channel::channel_session_params(
        &pubkey_hash(&session),
        FAR_FUTURE,
        &funding_lock,
        500,
        &[0u8; 20],
    );
    let args = sdk::registered_args(&pubkey_hash(&owner), &params);
    let lock_script = context.build_script(&lock_op, args).expect("script");

    let header = HeaderBuilder::default().timestamp(HEADER_TS_MS.pack()).build();
    context.insert_header(header.clone());
    let lock_dep = CellDep::new_builder().out_point(lock_op).build();
    let auth_dep = CellDep::new_builder().out_point(auth_op).build();
    let account_input = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script.clone())
            .build(),
        Bytes::new(),
    );

    let cfg = ChannelConfig {
        account_lock: lock_script,
        account_input,
        account_capacity: 1000,
        funding_lock,
        cell_deps: vec![lock_dep, auth_dep],
        header_dep: header.hash(),
    };

    // Drive the dev API: open (L1 funding tx) → pay×N (off-chain) → close.
    let mut chan = ChannelSession::new(cfg, MockRail::new());
    let open = chan.open(&"game-node".into(), 500).expect("open");
    for _ in 0..40 {
        chan.pay(5).expect("pay");
    }
    assert_eq!(chan.spent(), 200);

    // Session-sign the funding tx the API built and attach the witness.
    let session_sig = sign(&session, &sdk::tx_message(&open.tx));
    let witness =
        sdk::session_witness_registered(&session_sig, None, &sdk::channel::channel_proof_region());
    let tx = open.tx.as_advanced_builder().witness(witness.pack()).build();

    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("ChannelSession funding tx should pass the real lock");
    println!("sdk_channel_session_funding_tx cycles: {cycles}");

    let (settlement, _settle_tx) = chan.close().expect("close");
    assert_eq!(settlement.local, 300); // 500 budget - 200 spent
    assert_eq!(settlement.remote, 200);
}

#[test]
fn sdk_carried_session() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();

    let lock_script = context
        .build_script(&lock_op, sdk::owner_only_args(&pubkey_hash(&owner)))
        .expect("script");
    let sh = script_hash(&lock_script);

    let params = sdk::session_params(
        &pubkey_hash(&session),
        FAR_FUTURE,
        &sdk::WILDCARD_ROOT,
        sdk::SPEND_CAP_UNLIMITED,
        &[0u8; 20],
    );
    // account cell data empty => epoch 0.
    let owner_auth = sign(&owner, &sdk::session_auth_message(&sh, 0, &params));

    let header = HeaderBuilder::default().timestamp(HEADER_TS_MS.pack()).build();
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
        sdk::epoch_data(0),
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

    let session_sig = sign(&session, &sdk::tx_message(&tx));
    let witness = sdk::session_witness_carried(&params, &owner_auth, &session_sig, None, &[]);
    let tx = tx.as_advanced_builder().witness(witness.pack()).build();

    let cycles = context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("SDK-built carried session tx should pass");
    println!("sdk_carried_session cycles: {cycles}");
}
