#![no_std]
#![cfg_attr(not(test), no_main)]

//! Aggregator + game-cell **type script** for CKB — the shared-state half of the
//! Cartridge-style controller, adapted to the cell model.
//!
//! ## Why this exists
//! Starknet's Cartridge controller lets many controllers write one shared game
//! *contract*; the sequencer serialises concurrent writes. CKB has no shared
//! mutable contract storage — state is a cell, and a cell is one live outpoint,
//! so "everyone spends the shared cell" throttles to ~1 writer/block. The cell
//! model's adaptation is an **aggregator**: players submit session-signed
//! *intents*, an operator batches them into one tx that advances the game cell
//! N -> N+1, and THIS type script proves the transition is legitimate:
//!   - every intent is signed by the player's (session) key — via ckb-auth,
//!   - each intent is fresh (strictly increasing per-player nonce = no replay),
//!   - the resulting state is EXACTLY f(old state, intents) — the operator may
//!     order/censor (liveness) but can neither forge a move nor tamper the result
//!     (safety). Who may sequence is decided by the cell's LOCK, not here.
//!
//! ## Cell shape
//! - type args: `game_id` (32 bytes) — makes each game a distinct singleton.
//! - data (the shared state):
//!     `seq(8 LE) ‖ player_count(4 LE) ‖ [ pubkey_hash(20) ‖ score(8 LE) ‖ nonce(8 LE) ]*`
//! - Grouping by identical script means a transition is exactly one GroupInput +
//!   one GroupOutput (both carry the same game_id); this implicitly pins game_id.
//!
//! ## Intent batch (in the input cell's witness `input_type`)
//!     `n(2 LE) ‖ [ pubkey_hash(20) ‖ points(8 LE) ‖ nonce(8 LE) ‖ sig(65) ]*`
//! Each intent signs
//!     `blake2b_256(DOMAIN ‖ game_id(32) ‖ player(20) ‖ points(8 LE) ‖ nonce(8 LE))`
//! so a signature is bound to this game and this exact move (nonce), and cannot be
//! replayed (nonce must be prev+1) or reused on another game.
//!
//! The transition RULE here is a deliberately trivial, game-agnostic scoreboard
//! ("player scores `points`", `points <= MAX_POINTS_PER_MOVE`) — the authorization
//! + anti-replay + operator-honesty core is the reusable primitive; real game
//! rules drop into `apply_intent` without touching it.

#[cfg(test)]
extern crate alloc;

#[cfg(not(test))]
use ckb_std::default_alloc;
#[cfg(not(test))]
ckb_std::entry!(program_entry);
#[cfg(not(test))]
default_alloc!();

use alloc::ffi::CString;
use ckb_std::{
    ckb_constants::Source,
    ckb_types::{bytes::Bytes, core::ScriptHashType, prelude::*},
    error::SysError,
    high_level::{
        load_cell_data, load_cell_type_hash, load_script, load_script_hash, load_witness_args,
    },
    syscalls::wait,
};
use ckb_std::high_level::spawn_cell;
use hex::encode;

// THE GAME RULES — the shared crate compiled into both this type script and the
// off-chain stack (sdk -> wasm -> browser), so the enforced rule and the
// client's precomputation cannot drift. Edit game-rules/src/lib.rs to change
// the game; this file is only the on-chain FRAMING (tx shape, witness reading,
// signature verification, state comparison).
use controller_game_rules::{decode_batch, intent_message, GameError, GameState, Intent, GAME_ID_LEN};

include!(concat!(env!("OUT_DIR"), "/auth_code_hash.rs"));

// ckb-auth algorithm id: 0 = CKB secp256k1 (recoverable), same as the lock.
const AUTH_ID_CKB_SECP256K1: u8 = 0;

#[repr(i8)]
#[cfg_attr(test, derive(Debug))]
pub enum Error {
    IndexOutOfBound = 1,
    ItemMissing,
    LengthNotEnough,
    Encoding,
    // -- customized --
    BadTxShape,       // not a valid genesis or single-step transition
    ArgsLenError,     // game_id must be 32 bytes
    GenesisNotEmpty,  // a freshly created game must start from empty state
    StateMalformed,   // cell data isn't a well-formed state blob
    BatchMissing,     // transition witness carried no intent batch
    BatchMalformed,   // intent batch bytes don't parse
    NonceNotFresh,    // replayed / out-of-order intent (nonce != prev+1)
    PointsTooHigh,    // move violates the game rule
    Overflow,         // score/seq arithmetic overflow
    StateMismatch,    // output state != f(input state, intents)
    AuthError,        // ckb-auth rejected an intent signature
}

impl From<SysError> for Error {
    fn from(err: SysError) -> Self {
        match err {
            SysError::IndexOutOfBound => Self::IndexOutOfBound,
            SysError::ItemMissing => Self::ItemMissing,
            SysError::LengthNotEnough(_) => Self::LengthNotEnough,
            SysError::Encoding => Self::Encoding,
            SysError::Unknown(code) => panic!("unexpected sys error {}", code),
            _ => panic!("unreachable spawn-related sys error"),
        }
    }
}

/// Map shared-rules errors onto this script's exit codes (numbers are part of
/// the observable on-chain interface — see wire-formats.md §10).
impl From<GameError> for Error {
    fn from(err: GameError) -> Self {
        match err {
            GameError::StateMalformed => Self::StateMalformed,
            GameError::BatchMalformed => Self::BatchMalformed,
            GameError::NonceNotFresh => Self::NonceNotFresh,
            GameError::PointsTooHigh => Self::PointsTooHigh,
            GameError::Overflow => Self::Overflow,
        }
    }
}

pub fn program_entry() -> i8 {
    match verify() {
        Ok(_) => 0,
        Err(err) => err as i8,
    }
}

fn verify() -> Result<(), Error> {
    let script = load_script()?;
    let args: Bytes = script.args().unpack();
    let game_id: [u8; GAME_ID_LEN] = args.as_ref().try_into().map_err(|_| Error::ArgsLenError)?;

    let in_count = count_group_cells(Source::GroupInput)?;
    let out_count = count_group_cells(Source::GroupOutput)?;

    match (in_count, out_count) {
        // Genesis: the game cell is created. Must start from empty state.
        (0, 1) => verify_genesis(),
        // Transition: advance the single game cell by the witness's intent batch.
        (1, 1) => verify_transition(&game_id),
        // Anything else (multi-cell, or destruction) is not a legal game step.
        _ => Err(Error::BadTxShape),
    }
}

/// A freshly minted game must begin at seq 0 with no players — nobody may seed a
/// game with pre-credited scores.
fn verify_genesis() -> Result<(), Error> {
    let data = load_cell_data(0, Source::GroupOutput)?;
    let state = GameState::decode(&data)?;
    if state.seq != 0 || !state.players.is_empty() {
        return Err(Error::GenesisNotEmpty);
    }
    Ok(())
}

/// Advance the game cell: apply the witness's session-signed intent batch to the
/// input state (the SHARED rules crate's `apply_batch` — the same code the
/// client runs) and require the output state to match exactly.
fn verify_transition(game_id: &[u8; GAME_ID_LEN]) -> Result<(), Error> {
    let in_data = load_cell_data(0, Source::GroupInput)?;
    let out_data = load_cell_data(0, Source::GroupOutput)?;
    let mut state = GameState::decode(&in_data)?;
    let expected_out = GameState::decode(&out_data)?;

    // The intent batch rides in the input game cell's witness (input_type field),
    // read by ABSOLUTE input index (type scripts have no per-group witness).
    let gi = find_group_input_index()?.ok_or(Error::BadTxShape)?;
    let witness = load_witness_args(gi, Source::Input)?;
    let batch: Bytes = witness
        .input_type()
        .to_opt()
        .ok_or(Error::BatchMissing)?
        .unpack();

    let intents = decode_batch(&batch)?;

    // Only the chain can check signatures (ckb-auth spawn) — do it per intent.
    for intent in &intents {
        verify_intent_sig(game_id, intent)?;
    }

    // The rule itself + the seq bump: exactly the shared transition function.
    state.apply_batch(&intents)?;

    // The operator must have produced exactly f(input, intents) — no tampering.
    if !state.equals_unordered(&expected_out) {
        return Err(Error::StateMismatch);
    }
    Ok(())
}

/// Check one intent's signature via ckb-auth, over the shared crate's
/// domain-separated intent message (the exact bytes the client signed).
fn verify_intent_sig(game_id: &[u8; GAME_ID_LEN], intent: &Intent) -> Result<(), Error> {
    let message = intent_message(game_id, &intent.hash, intent.points, intent.nonce);
    verify_signature(AUTH_ID_CKB_SECP256K1, &intent.sig, &message, &intent.hash)
}

// --- helpers ----------------------------------------------------------------

/// Count cells in a script group (GroupInput / GroupOutput) by walking data.
fn count_group_cells(source: Source) -> Result<usize, Error> {
    let mut i = 0usize;
    loop {
        match load_cell_data(i, source) {
            Ok(_) => i += 1,
            Err(SysError::IndexOutOfBound) => return Ok(i),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Absolute input index of the (single) cell carrying this type script — needed to
/// read its witness (type scripts have no per-group witness).
fn find_group_input_index() -> Result<Option<usize>, Error> {
    let me = load_script_hash()?;
    let mut i = 0usize;
    loop {
        match load_cell_type_hash(i, Source::Input) {
            Ok(Some(h)) if h == me => return Ok(Some(i)),
            Ok(_) => i += 1,
            Err(SysError::IndexOutOfBound) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Delegate signature verification to the ckb-auth binary via spawn_cell (same
/// pattern the controller session lock uses).
fn verify_signature(
    algorithm_id: u8,
    signature: &[u8],
    message: &[u8; 32],
    pubkey_hash: &[u8],
) -> Result<(), Error> {
    let algorithm_id_str = CString::new(encode([algorithm_id])).unwrap();
    let signature_str = CString::new(encode(signature)).unwrap();
    let message_str = CString::new(encode(message)).unwrap();
    let pubkey_hash_str = CString::new(encode(pubkey_hash)).unwrap();

    let auth_args = [
        algorithm_id_str.as_c_str(),
        signature_str.as_c_str(),
        message_str.as_c_str(),
        pubkey_hash_str.as_c_str(),
    ];

    let pid = spawn_cell(&AUTH_CODE_HASH, ScriptHashType::Data1, &auth_args, &[])
        .map_err(|_| Error::AuthError)?;
    let result = wait(pid).map_err(|_| Error::AuthError)?;
    if result != 0 {
        return Err(Error::AuthError);
    }
    Ok(())
}

