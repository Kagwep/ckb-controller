//! In-VM tests for the aggregator + game-cell TYPE script, run in CKB-VM via
//! ckb-testtool. These exercise the real syscall paths the host unit tests can't:
//! genesis vs transition tx shapes, reading the intent batch from the input cell's
//! witness, and per-intent signature verification via spawn_cell -> ckb-auth.
//!
//! Prereqs: `./build.sh` (or `make build`) produced
//! ../build/release/controller-game-cell, and ckb-auth is at ../deps/auth.
//!
//! Model recap: a game cell (type = this script, args = 32-byte game_id) is
//! advanced N -> N+1 by an operator that batches player intents. The cell's LOCK
//! (here ALWAYS_SUCCESS = "any operator may sequence") decides liveness; this TYPE
//! script decides safety — each intent is session-signed, fresh (nonce = prev+1),
//! and the output state is exactly f(input, intents).

use ckb_testtool::builtin::ALWAYS_SUCCESS;
use ckb_testtool::ckb_crypto::secp::{Generator, Privkey};
use ckb_testtool::ckb_hash::blake2b_256;
use ckb_testtool::ckb_types::{
    bytes::Bytes,
    core::{TransactionBuilder, TransactionView},
    packed::*,
    prelude::*,
};
use ckb_testtool::context::Context;
use std::fs;

const MAX_CYCLES: u64 = 100_000_000;
const MAX_POINTS_PER_MOVE: u64 = 1000;
const INTENT_DOMAIN: &[u8] = b"ckb-controller/game-intent/v1";

fn game_binary() -> Bytes {
    fs::read("../build/release/controller-game-cell")
        .expect("build the contracts first: `./build.sh` or `make build`")
        .into()
}

fn auth_binary() -> Bytes {
    fs::read("../deps/auth")
        .expect("place the ckb-auth binary at deps/auth")
        .into()
}

fn pubkey_hash(privkey: &Privkey) -> [u8; 20] {
    let pubkey = privkey.pubkey().expect("pubkey");
    let h = blake2b_256(pubkey.serialize());
    h[0..20].try_into().unwrap()
}

/// The message a player signs for an intent — must match the contract's
/// `intent_message`: blake2b_256(DOMAIN ‖ game_id ‖ player ‖ points ‖ nonce).
fn intent_message(game_id: &[u8], player: &[u8; 20], points: u64, nonce: u64) -> [u8; 32] {
    let mut buf = Vec::new();
    buf.extend_from_slice(INTENT_DOMAIN);
    buf.extend_from_slice(game_id);
    buf.extend_from_slice(player);
    buf.extend_from_slice(&points.to_le_bytes());
    buf.extend_from_slice(&nonce.to_le_bytes());
    blake2b_256(&buf)
}

fn sign(privkey: &Privkey, message: &[u8; 32]) -> Vec<u8> {
    privkey
        .sign_recoverable(&(*message).into())
        .expect("sign")
        .serialize()
}

/// Encode the game state blob: seq ‖ count ‖ [hash ‖ score ‖ nonce]*.
fn encode_state(seq: u64, players: &[([u8; 20], u64, u64)]) -> Bytes {
    let mut d = Vec::new();
    d.extend_from_slice(&seq.to_le_bytes());
    d.extend_from_slice(&(players.len() as u32).to_le_bytes());
    for (h, score, nonce) in players {
        d.extend_from_slice(h);
        d.extend_from_slice(&score.to_le_bytes());
        d.extend_from_slice(&nonce.to_le_bytes());
    }
    d.into()
}

/// Encode one intent: player ‖ points ‖ nonce ‖ sig(65).
fn encode_intent(player: &[u8; 20], points: u64, nonce: u64, sig: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(101);
    b.extend_from_slice(player);
    b.extend_from_slice(&points.to_le_bytes());
    b.extend_from_slice(&nonce.to_le_bytes());
    b.extend_from_slice(sig);
    assert_eq!(b.len(), 101, "intent must be 101 bytes");
    b
}

/// Frame a batch of encoded intents: n(2 LE) ‖ concat(intents).
fn encode_batch(intents: &[Vec<u8>]) -> Bytes {
    let mut b = Vec::new();
    b.extend_from_slice(&(intents.len() as u16).to_le_bytes());
    for i in intents {
        b.extend_from_slice(i);
    }
    b.into()
}

fn witness_input_type(batch: Bytes) -> Bytes {
    WitnessArgs::new_builder()
        .input_type(Some(batch).pack())
        .build()
        .as_bytes()
}

/// A fully-wired test scaffold: deploys the game type script, ckb-auth, and
/// always-success (the game cell's lock), returns the context + reusable pieces.
struct Scaffold {
    context: Context,
    game_op: OutPoint,
    auth_op: OutPoint,
    always_op: OutPoint,
    always_script: Script,
}

fn scaffold() -> Scaffold {
    let mut context = Context::default();
    let game_op = context.deploy_cell(game_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let always_op = context.deploy_cell(ALWAYS_SUCCESS.clone());
    let always_script = context
        .build_script(&always_op, Bytes::new())
        .expect("always-success script");
    Scaffold { context, game_op, auth_op, always_op, always_script }
}

impl Scaffold {
    fn game_script(&mut self, game_id: &[u8; 32]) -> Script {
        self.context
            .build_script(&self.game_op, Bytes::from(game_id.to_vec()))
            .expect("game type script")
    }

    fn cell_deps(&self) -> CellDepVec {
        vec![
            CellDep::new_builder().out_point(self.game_op.clone()).build(),
            CellDep::new_builder().out_point(self.auth_op.clone()).build(),
            CellDep::new_builder().out_point(self.always_op.clone()).build(),
        ]
        .pack()
    }

    /// A game cell (lock = always-success, type = game script) with the given data.
    fn game_cell(&self, game_script: &Script) -> CellOutput {
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(self.always_script.clone())
            .type_(Some(game_script.clone()).pack())
            .build()
    }

    /// A plain always-success cell (no type) — funds a genesis tx's inputs.
    fn plain_cell(&self) -> CellOutput {
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(self.always_script.clone())
            .build()
    }
}

// --- genesis ----------------------------------------------------------------

/// Build a genesis tx: a plain input funds a newly created game cell with `data`.
fn genesis_tx(s: &mut Scaffold, game_id: &[u8; 32], data: Bytes) -> TransactionView {
    let game_script = s.game_script(game_id);
    let plain = s.plain_cell();
    let input_op = s.context.create_cell(plain, Bytes::new());
    let out = s.game_cell(&game_script);
    TransactionBuilder::default()
        .cell_deps(s.cell_deps())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .output(out)
        .output_data(data.pack())
        .witness(Bytes::new().pack())
        .build()
}

#[test]
fn genesis_creates_empty_game() {
    let mut s = scaffold();
    let game_id = [7u8; 32];
    let tx = genesis_tx(&mut s, &game_id, encode_state(0, &[]));
    let cycles = s
        .context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("empty genesis should pass");
    println!("genesis_creates_empty_game cycles: {cycles}");
}

#[test]
fn genesis_accepts_zero_length_data() {
    let mut s = scaffold();
    let game_id = [7u8; 32];
    // zero-length data is a valid empty state (seq 0, no players).
    let tx = genesis_tx(&mut s, &game_id, Bytes::new());
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("zero-length genesis should pass");
}

#[test]
fn genesis_with_prefilled_state_rejected() {
    let mut s = scaffold();
    let game_id = [7u8; 32];
    // a game that mints itself with a player already scored 500 — must be rejected.
    let tx = genesis_tx(&mut s, &game_id, encode_state(0, &[([1u8; 20], 500, 1)]));
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("genesis with pre-credited scores must be rejected");
}

#[test]
fn genesis_with_nonzero_seq_rejected() {
    let mut s = scaffold();
    let game_id = [7u8; 32];
    let tx = genesis_tx(&mut s, &game_id, encode_state(9, &[]));
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("genesis with nonzero seq must be rejected");
}

// --- transitions ------------------------------------------------------------

/// Build a transition tx: consume a game cell with `in_state`, produce one with
/// `out_state`, carrying `batch` in the input cell's witness.
fn transition_tx(
    s: &mut Scaffold,
    game_id: &[u8; 32],
    in_state: Bytes,
    out_state: Bytes,
    batch: Bytes,
) -> TransactionView {
    let game_script = s.game_script(game_id);
    let in_cell = s.game_cell(&game_script);
    let input_op = s.context.create_cell(in_cell, in_state);
    let out = s.game_cell(&game_script);
    TransactionBuilder::default()
        .cell_deps(s.cell_deps())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .output(out)
        .output_data(out_state.pack())
        .witness(witness_input_type(batch).pack())
        .build()
}

#[test]
fn single_move_from_empty_applies() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    let player = Generator::new().gen_privkey();
    let ph = pubkey_hash(&player);

    let sig = sign(&player, &intent_message(&game_id, &ph, 10, 1));
    let batch = encode_batch(&[encode_intent(&ph, 10, 1, &sig)]);

    let in_state = encode_state(0, &[]);
    let out_state = encode_state(1, &[(ph, 10, 1)]);

    let tx = transition_tx(&mut s, &game_id, in_state, out_state, batch);
    let cycles = s
        .context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("a single signed move should pass");
    println!("single_move_from_empty_applies cycles: {cycles}");
}

#[test]
fn batch_two_players_applies() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    let a = Generator::new().gen_privkey();
    let b = Generator::new().gen_privkey();
    let (ah, bh) = (pubkey_hash(&a), pubkey_hash(&b));

    let sa = sign(&a, &intent_message(&game_id, &ah, 30, 1));
    let sb = sign(&b, &intent_message(&game_id, &bh, 20, 1));
    let batch = encode_batch(&[
        encode_intent(&ah, 30, 1, &sa),
        encode_intent(&bh, 20, 1, &sb),
    ]);

    let in_state = encode_state(0, &[]);
    let out_state = encode_state(2, &[(ah, 30, 1), (bh, 20, 1)]);

    let tx = transition_tx(&mut s, &game_id, in_state, out_state, batch);
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("a two-player batch should pass");
}

#[test]
fn existing_player_accumulates() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    let p = Generator::new().gen_privkey();
    let ph = pubkey_hash(&p);

    // player already at score 10, nonce 1; a second move (+5, nonce 2).
    let sig = sign(&p, &intent_message(&game_id, &ph, 5, 2));
    let batch = encode_batch(&[encode_intent(&ph, 5, 2, &sig)]);

    let in_state = encode_state(4, &[(ph, 10, 1)]);
    let out_state = encode_state(5, &[(ph, 15, 2)]);

    let tx = transition_tx(&mut s, &game_id, in_state, out_state, batch);
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("an accumulating move should pass");
}

#[test]
fn forged_signature_rejected() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    let player = Generator::new().gen_privkey();
    let attacker = Generator::new().gen_privkey();
    let ph = pubkey_hash(&player);

    // attacker signs but the intent claims to be the player — ckb-auth must reject.
    let sig = sign(&attacker, &intent_message(&game_id, &ph, 10, 1));
    let batch = encode_batch(&[encode_intent(&ph, 10, 1, &sig)]);

    let in_state = encode_state(0, &[]);
    let out_state = encode_state(1, &[(ph, 10, 1)]);

    let tx = transition_tx(&mut s, &game_id, in_state, out_state, batch);
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("a forged intent signature must be rejected");
}

#[test]
fn wrong_game_id_in_signature_rejected() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    let other_game = [9u8; 32];
    let player = Generator::new().gen_privkey();
    let ph = pubkey_hash(&player);

    // player signs a valid intent, but for a DIFFERENT game — must not replay here.
    let sig = sign(&player, &intent_message(&other_game, &ph, 10, 1));
    let batch = encode_batch(&[encode_intent(&ph, 10, 1, &sig)]);

    let tx = transition_tx(
        &mut s,
        &game_id,
        encode_state(0, &[]),
        encode_state(1, &[(ph, 10, 1)]),
        batch,
    );
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("an intent signed for another game must be rejected");
}

#[test]
fn replayed_nonce_rejected() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    let p = Generator::new().gen_privkey();
    let ph = pubkey_hash(&p);

    // player is already at nonce 1; the operator replays a nonce-1 intent.
    let sig = sign(&p, &intent_message(&game_id, &ph, 10, 1));
    let batch = encode_batch(&[encode_intent(&ph, 10, 1, &sig)]);

    let in_state = encode_state(1, &[(ph, 10, 1)]);
    // even if the operator "expected" some output, a stale nonce fails first.
    let out_state = encode_state(2, &[(ph, 20, 1)]);

    let tx = transition_tx(&mut s, &game_id, in_state, out_state, batch);
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("a replayed (stale-nonce) intent must be rejected");
}

#[test]
fn tampered_output_score_rejected() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    let p = Generator::new().gen_privkey();
    let ph = pubkey_hash(&p);

    // a legitimately-signed +10 move, but the operator writes +999 to the output.
    let sig = sign(&p, &intent_message(&game_id, &ph, 10, 1));
    let batch = encode_batch(&[encode_intent(&ph, 10, 1, &sig)]);

    let in_state = encode_state(0, &[]);
    let out_state = encode_state(1, &[(ph, 999, 1)]); // tampered

    let tx = transition_tx(&mut s, &game_id, in_state, out_state, batch);
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("operator tampering the output state must be rejected");
}

#[test]
fn tampered_output_seq_rejected() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    let p = Generator::new().gen_privkey();
    let ph = pubkey_hash(&p);

    let sig = sign(&p, &intent_message(&game_id, &ph, 10, 1));
    let batch = encode_batch(&[encode_intent(&ph, 10, 1, &sig)]);

    // one intent applied but seq jumps by 2.
    let tx = transition_tx(
        &mut s,
        &game_id,
        encode_state(0, &[]),
        encode_state(2, &[(ph, 10, 1)]),
        batch,
    );
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("a wrong output seq must be rejected");
}

#[test]
fn points_over_max_rejected() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    let p = Generator::new().gen_privkey();
    let ph = pubkey_hash(&p);

    let points = MAX_POINTS_PER_MOVE + 1;
    let sig = sign(&p, &intent_message(&game_id, &ph, points, 1));
    let batch = encode_batch(&[encode_intent(&ph, points, 1, &sig)]);

    let tx = transition_tx(
        &mut s,
        &game_id,
        encode_state(0, &[]),
        encode_state(1, &[(ph, points, 1)]),
        batch,
    );
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("a move exceeding the per-move rule cap must be rejected");
}

#[test]
fn two_output_game_cells_rejected() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    let game_script = s.game_script(&game_id);

    // A transition that tries to fan the game into TWO cells — bad shape.
    let in_cell = s.game_cell(&game_script);
    let input_op = s.context.create_cell(in_cell, encode_state(0, &[]));
    let tx = TransactionBuilder::default()
        .cell_deps(s.cell_deps())
        .input(CellInput::new_builder().previous_output(input_op).build())
        .outputs(vec![s.game_cell(&game_script), s.game_cell(&game_script)])
        .outputs_data(vec![encode_state(0, &[]), encode_state(0, &[])].pack())
        .witness(witness_input_type(encode_batch(&[])).pack())
        .build();
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect_err("splitting the game into two cells must be rejected");
}

#[test]
fn empty_batch_is_a_noop_transition() {
    let mut s = scaffold();
    let game_id = [3u8; 32];
    // no intents: state must be byte-identical (seq unchanged).
    let tx = transition_tx(
        &mut s,
        &game_id,
        encode_state(5, &[([1u8; 20], 10, 1)]),
        encode_state(5, &[([1u8; 20], 10, 1)]),
        encode_batch(&[]),
    );
    s.context
        .verify_tx(&tx, MAX_CYCLES)
        .expect("an empty batch that changes nothing should pass");
}
