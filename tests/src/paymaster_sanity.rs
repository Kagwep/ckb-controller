//! End-to-end paymaster sanity: run the full sponsored-relay handshake — issue a
//! capability token, gate it, assemble-then-balance the client's partial tx, have
//! the client session-sign last (via the SDK), and verify the result against the
//! REAL controller lock in CKB-VM. The game pays no gas; the paymaster's fee cell
//! covers it.

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
use controller_paymaster::{
    authz::Capability,
    authz::{Ed25519Authority, Ed25519Gate},
    biscuit_gate::{BiscuitAuthority, BiscuitGate},
    Paymaster, PaymasterError,
};
use controller_sdk as sdk;
use std::fs;

const MAX_CYCLES: u64 = 100_000_000;
const FAR_FUTURE: u64 = 2_000_000_000;
const NOW: u64 = 1_700_000_000;
const SCOPE: &str = "ckb-controller-sponsor";

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

#[test]
fn paymaster_sponsors_session_tx() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let as_op = context.deploy_cell(ALWAYS_SUCCESS.clone());

    // --- relayer trust root: issues + verifies sponsor tokens ---
    let authority = Ed25519Authority::from_seed(&[7u8; 32]);
    let paymaster = Paymaster::new(Ed25519Gate::new(authority.public_key()).unwrap());
    let token = authority.issue(&Capability {
        scope: SCOPE.into(),
        subject: "player-1".into(),
        not_after: FAR_FUTURE,
    });

    // --- client builds a partial session tx (no fee, unsigned) ---
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();
    let params = sdk::session_params(
        &pubkey_hash(&session),
        FAR_FUTURE,
        &sdk::WILDCARD_ROOT,
        sdk::SPEND_CAP_UNLIMITED,
        &[0u8; 20],
    );
    let lock_script = context
        .build_script(&lock_op, sdk::registered_args(&pubkey_hash(&owner), &params))
        .expect("script");

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());

    let account_input = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        Bytes::new(),
    );
    let partial = TransactionBuilder::default()
        .cell_dep(CellDep::new_builder().out_point(lock_op).build())
        .cell_dep(CellDep::new_builder().out_point(auth_op).build())
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(account_input).build())
        // a "game state" output the session pays into (plain recipient)
        .output(
            CellOutput::new_builder()
                .capacity(1000u64.pack())
                .lock(Script::new_builder().args(Bytes::from("game").pack()).build())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    // --- relayer's fee cell (always-success lock) ---
    let always_success = context.build_script(&as_op, Bytes::new()).unwrap();
    let fee_cell = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(always_success.clone())
            .build(),
        Bytes::new(),
    );
    let as_dep = CellDep::new_builder().out_point(as_op).build();

    // --- paymaster: gate the token, then assemble-then-balance ---
    let balanced = paymaster
        .sponsor(
            &token,
            NOW,
            SCOPE,
            &partial,
            fee_cell,
            1000, // fee cell capacity
            100,  // fee paid
            always_success, // change back to the relayer
            &[as_dep],
        )
        .expect("authorized sponsorship should balance the tx");

    // structure: account input (0) + fee input (1); game output + change output.
    assert_eq!(balanced.inputs().len(), 2);
    assert_eq!(balanced.outputs().len(), 2);

    // --- client signs LAST over the final (balanced) tx ---
    let session_sig = sign(&session, &sdk::tx_message(&balanced));
    let account_witness = sdk::session_witness_registered(&session_sig, None, &[]);
    let signed = balanced
        .as_advanced_builder()
        .set_witnesses(vec![account_witness.pack(), Bytes::new().pack()])
        .build();

    let cycles = context
        .verify_tx(&signed, MAX_CYCLES)
        .expect("paymaster-sponsored session tx should pass");
    println!("paymaster_sponsors_session_tx cycles: {cycles}");
}

/// Hold a *gameplay session*: the owner approves ONE session (by registering it
/// in the account args), then the game plays many actions in a row — each
/// session-signed silently (no owner involvement), each sponsored by the
/// paymaster (no gas to the player), each spending within the per-tx spend cap.
/// This is the loop a game runs after "connect".
#[test]
fn gameplay_session_loop() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let as_op = context.deploy_cell(ALWAYS_SUCCESS.clone());

    // relayer trust root + token (issued once, like the on-chain session).
    let authority = Ed25519Authority::from_seed(&[7u8; 32]);
    let paymaster = Paymaster::new(Ed25519Gate::new(authority.public_key()).unwrap());
    let token = authority.issue(&Capability {
        scope: SCOPE.into(),
        subject: "player-1".into(),
        not_after: FAR_FUTURE,
    });

    // ONE owner approval: the session (key + caveats) is baked into the account
    // args. Per-action, only the session key signs.
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();
    const MOVE: u64 = 1000; // value spent per action
    let params = sdk::session_params(
        &pubkey_hash(&session),
        FAR_FUTURE,
        &sdk::WILDCARD_ROOT,
        MOVE as u128, // spend cap == per-action move
        &[0u8; 20],
    );
    let lock_script = context
        .build_script(&lock_op, sdk::registered_args(&pubkey_hash(&owner), &params))
        .expect("script");
    let always_success = context.build_script(&as_op, Bytes::new()).unwrap();
    let as_dep = CellDep::new_builder().out_point(as_op).build();

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());

    const ACTIONS: u64 = 5;
    let start: u64 = 100_000;

    for i in 0..ACTIONS {
        // The account cell as it stands before action i (on a real chain this is
        // the previous action's output; here we materialise the current state).
        let account_cap = start - i * MOVE;
        let account_input = context.create_cell(
            CellOutput::new_builder()
                .capacity(account_cap.pack())
                .lock(lock_script.clone())
                .build(),
            Bytes::new(),
        );

        // Action: spend MOVE into a fresh game-state cell, recreate the account
        // cell (same args + data) with the remainder.
        let partial = TransactionBuilder::default()
            .cell_dep(CellDep::new_builder().out_point(lock_op.clone()).build())
            .cell_dep(CellDep::new_builder().out_point(auth_op.clone()).build())
            .header_dep(header.hash())
            .input(CellInput::new_builder().previous_output(account_input).build())
            .output(
                CellOutput::new_builder()
                    .capacity((account_cap - MOVE).pack())
                    .lock(lock_script.clone()) // account continues (same args + empty data)
                    .build(),
            )
            .output(
                CellOutput::new_builder()
                    .capacity(MOVE.pack())
                    .lock(
                        Script::new_builder()
                            .args(Bytes::from(format!("move-{i}")).pack())
                            .build(),
                    )
                    .build(),
            )
            .outputs_data(vec![Bytes::new(), Bytes::new()].pack())
            .build();

        // Paymaster sponsors a fresh fee cell each action.
        let fee_cell = context.create_cell(
            CellOutput::new_builder()
                .capacity(1000u64.pack())
                .lock(always_success.clone())
                .build(),
            Bytes::new(),
        );
        let balanced = paymaster
            .sponsor(
                &token, NOW, SCOPE, &partial, fee_cell, 1000, 100, always_success.clone(), &[as_dep.clone()],
            )
            .expect("authorized");

        // The game signs the action silently with the session key — no owner.
        let session_sig = sign(&session, &sdk::tx_message(&balanced));
        let account_witness = sdk::session_witness_registered(&session_sig, None, &[]);
        let signed = balanced
            .as_advanced_builder()
            .set_witnesses(vec![account_witness.pack(), Bytes::new().pack()])
            .build();

        context
            .verify_tx(&signed, MAX_CYCLES)
            .unwrap_or_else(|e| panic!("gameplay action {i} should pass: {e}"));
    }

    println!("held a {ACTIONS}-action gameplay session on a single owner approval");
}

/// Same end-to-end sponsored relay as `paymaster_sponsors_session_tx`, but gated
/// by the **production** `biscuit-auth` gate (the one Fiber uses) instead of the
/// minimal Ed25519 reference. Proves a real biscuit token sponsors a real session
/// tx that the real lock accepts in CKB-VM — the gate is swappable end-to-end.
#[test]
fn biscuit_gate_sponsors_session_tx() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let as_op = context.deploy_cell(ALWAYS_SUCCESS.clone());

    // relayer trust root: a biscuit authority issues a base64 sponsor token.
    let authority = BiscuitAuthority::from_seed(&[7u8; 32]).unwrap();
    let paymaster = Paymaster::new(BiscuitGate::new(&authority.public_key()).unwrap());
    let token = authority
        .issue(&Capability {
            scope: SCOPE.into(),
            subject: "player-1".into(),
            not_after: FAR_FUTURE,
        })
        .unwrap();

    // client builds a partial session tx (no fee, unsigned).
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();
    let params = sdk::session_params(
        &pubkey_hash(&session),
        FAR_FUTURE,
        &sdk::WILDCARD_ROOT,
        sdk::SPEND_CAP_UNLIMITED,
        &[0u8; 20],
    );
    let lock_script = context
        .build_script(&lock_op, sdk::registered_args(&pubkey_hash(&owner), &params))
        .expect("script");

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());

    let account_input = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        Bytes::new(),
    );
    let partial = TransactionBuilder::default()
        .cell_dep(CellDep::new_builder().out_point(lock_op).build())
        .cell_dep(CellDep::new_builder().out_point(auth_op).build())
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(account_input).build())
        .output(
            CellOutput::new_builder()
                .capacity(1000u64.pack())
                .lock(Script::new_builder().args(Bytes::from("game").pack()).build())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let always_success = context.build_script(&as_op, Bytes::new()).unwrap();
    let fee_cell = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(always_success.clone())
            .build(),
        Bytes::new(),
    );
    let as_dep = CellDep::new_builder().out_point(as_op).build();

    // gate the biscuit token (passed as bytes), then assemble-then-balance.
    let balanced = paymaster
        .sponsor(
            token.as_bytes(),
            NOW,
            SCOPE,
            &partial,
            fee_cell,
            1000,
            100,
            always_success,
            &[as_dep],
        )
        .expect("biscuit-authorized sponsorship should balance the tx");

    // client signs LAST over the balanced tx; the real lock verifies in CKB-VM.
    let session_sig = sign(&session, &sdk::tx_message(&balanced));
    let account_witness = sdk::session_witness_registered(&session_sig, None, &[]);
    let signed = balanced
        .as_advanced_builder()
        .set_witnesses(vec![account_witness.pack(), Bytes::new().pack()])
        .build();

    let cycles = context
        .verify_tx(&signed, MAX_CYCLES)
        .expect("biscuit-gated session tx should pass");
    println!("biscuit_gate_sponsors_session_tx cycles: {cycles}");
}

/// The production gate must refuse an expired biscuit before any tx work — same
/// reject contract as the Ed25519 gate.
#[test]
fn biscuit_gate_refuses_unauthorized() {
    let authority = BiscuitAuthority::from_seed(&[7u8; 32]).unwrap();
    let paymaster = Paymaster::new(BiscuitGate::new(&authority.public_key()).unwrap());
    let token = authority
        .issue(&Capability {
            scope: SCOPE.into(),
            subject: "player-1".into(),
            not_after: NOW - 1, // already expired at NOW
        })
        .unwrap();
    let partial = TransactionBuilder::default().build();
    let err = paymaster
        .sponsor(
            token.as_bytes(),
            NOW,
            SCOPE,
            &partial,
            OutPoint::new(Default::default(), 0),
            1000,
            100,
            Script::default(),
            &[],
        )
        .expect_err("expired biscuit must not be sponsored");
    assert!(matches!(err, PaymasterError::Unauthorized(_)));
}

#[test]
fn paymaster_refuses_unauthorized() {
    let authority = Ed25519Authority::from_seed(&[7u8; 32]);
    let paymaster = Paymaster::new(Ed25519Gate::new(authority.public_key()).unwrap());
    // token already expired at NOW.
    let token = authority.issue(&Capability {
        scope: SCOPE.into(),
        subject: "player-1".into(),
        not_after: NOW - 1,
    });
    let partial = TransactionBuilder::default().build();
    let err = paymaster
        .sponsor(
            &token,
            NOW,
            SCOPE,
            &partial,
            OutPoint::new(Default::default(), 0),
            1000,
            100,
            Script::default(),
            &[],
        )
        .expect_err("expired token must not be sponsored");
    assert!(matches!(err, PaymasterError::Unauthorized(_)));
}
