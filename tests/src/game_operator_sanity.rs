//! Drift guard for the aggregator: build transitions with the real off-chain
//! stack (`controller_sdk::game` + `paymaster_service::GameOperator`) and verify
//! them against the REAL game type-script binary in CKB-VM. If the SDK/operator
//! encoding ever drifts from the on-chain script, these fail.

use ckb_testtool::builtin::ALWAYS_SUCCESS;
use ckb_testtool::ckb_crypto::secp::{Generator, Privkey};
use ckb_testtool::ckb_hash::blake2b_256;
use ckb_testtool::ckb_types::{
    bytes::Bytes,
    packed::*,
    prelude::*,
};
use ckb_testtool::context::Context;
use controller_sdk::game::{intent_message, GameState, Intent, PlayerEntry};
use paymaster_service::{GameOperator, GameTip};
use std::fs;

const MAX_CYCLES: u64 = 100_000_000;

fn game_binary() -> Bytes {
    fs::read("../build/release/controller-game-cell")
        .expect("build the contracts first: ./build.sh")
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

/// Build a signed intent via the SDK message + a ckb-testtool secp key.
fn signed_intent(game_id: &[u8; 32], key: &Privkey, points: u64, nonce: u64) -> Intent {
    let hash = pubkey_hash(key);
    let msg = intent_message(game_id, &hash, points, nonce);
    let sig = key.sign_recoverable(&msg.into()).expect("sign").serialize();
    Intent { hash, points, nonce, sig: sig.try_into().expect("65-byte sig") }
}

/// Deploy the scripts and stand up an operator whose tip is a freshly created game
/// cell holding `initial`. Returns (context, operator, game_id).
fn operator_with_tip(initial: GameState) -> (Context, GameOperator, [u8; 32]) {
    let mut context = Context::default();
    let game_op = context.deploy_cell(game_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let always_op = context.deploy_cell(ALWAYS_SUCCESS.clone());
    let always = context.build_script(&always_op, Bytes::new()).unwrap();

    let game_id = [42u8; 32];
    let game_script = context
        .build_script(&game_op, Bytes::from(game_id.to_vec()))
        .unwrap();

    let output = CellOutput::new_builder()
        .capacity(1000u64.pack())
        .lock(always)
        .type_(Some(game_script).pack())
        .build();
    let tip_op = context.create_cell(output.clone(), Bytes::from(initial.encode()));

    let cell_deps = vec![
        CellDep::new_builder().out_point(game_op).build(),
        CellDep::new_builder().out_point(auth_op).build(),
        CellDep::new_builder().out_point(always_op).build(),
    ];

    let tip = GameTip { out_point: tip_op, output, state: initial };
    let operator = GameOperator::new(game_id, cell_deps, tip);
    (context, operator, game_id)
}

#[test]
fn operator_transition_verifies_from_empty() {
    let (context, mut op, game_id) = operator_with_tip(GameState::empty());
    let a = Generator::new().gen_privkey();
    let b = Generator::new().gen_privkey();

    op.submit(signed_intent(&game_id, &a, 30, 1)).unwrap();
    op.submit(signed_intent(&game_id, &b, 20, 1)).unwrap();

    let t = op.build_transition().expect("build");
    assert_eq!(t.applied, 2);
    assert_eq!(t.next_state.seq, 2);

    let cycles = context
        .verify_tx(&t.tx, MAX_CYCLES)
        .expect("operator-built transition must pass the real type script");
    println!("operator_transition_verifies_from_empty cycles: {cycles}");
}

#[test]
fn operator_transition_verifies_accumulate() {
    // tip already holds player A at score 10, nonce 1.
    let a = Generator::new().gen_privkey();
    let ah = pubkey_hash(&a);
    let initial = GameState {
        seq: 1,
        players: vec![PlayerEntry { hash: ah, score: 10, nonce: 1 }],
    };
    let (context, mut op, game_id) = operator_with_tip(initial);

    // A scores +5 (nonce 2).
    op.submit(signed_intent(&game_id, &a, 5, 2)).unwrap();
    let t = op.build_transition().expect("build");
    assert_eq!(t.next_state.seq, 2);
    assert_eq!(t.next_state.players[0].score, 15);

    context
        .verify_tx(&t.tx, MAX_CYCLES)
        .expect("accumulating transition must pass the real type script");
}

#[test]
fn operator_transition_with_forged_sig_rejected_on_chain() {
    // The operator can't tell a forged sig (it only sequences); the type script
    // must reject it — proving safety doesn't depend on the operator.
    let (context, mut op, game_id) = operator_with_tip(GameState::empty());
    let player = Generator::new().gen_privkey();
    let attacker = Generator::new().gen_privkey();

    // build an intent that claims `player` but is signed by `attacker`.
    let ph = pubkey_hash(&player);
    let msg = intent_message(&game_id, &ph, 10, 1);
    let bad_sig = attacker.sign_recoverable(&msg.into()).unwrap().serialize();
    // structurally valid (new player, nonce 1) so it's admitted; only the forged
    // SIGNATURE is bad, which the operator can't see — the type script catches it.
    op.submit(Intent { hash: ph, points: 10, nonce: 1, sig: bad_sig.try_into().unwrap() })
        .unwrap();

    let t = op.build_transition().expect("build (operator doesn't verify sigs)");
    context
        .verify_tx(&t.tx, MAX_CYCLES)
        .expect_err("the type script must reject a forged intent the operator relayed");
}
