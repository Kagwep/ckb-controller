//! The game's rules — state model, wire encodings, and the transition function.
//!
//! This crate is compiled TWICE: into the on-chain type script
//! (`contracts/controller-game-cell`) that verifies every transition, and into
//! the off-chain stack (`controller-sdk` → `controller-wasm` → the browser)
//! that precomputes them. Because both sides run this exact code, the rules
//! the chain enforces and the rules the client shows cannot drift.
//!
//! ## Making it YOUR game (the template surface)
//!
//! Everything game-specific is in the "GAME RULE" section below:
//!   - [`PlayerEntry`] / [`GameState`] — what the board is,
//!   - [`Intent`] — what a move is,
//!   - [`GameState::apply_intent`] — what a move DOES (the rule the type
//!     script enforces), plus its rule constants ([`MAX_POINTS_PER_MOVE`]).
//!
//! The framing around it (nonce anti-replay, batch encoding, the
//! domain-separated intent signing message) is the reusable aggregator
//! primitive — change it only together with the type script's verification
//! and docs/internals/wire-formats.md.
//!
//! Byte layouts (all integers little-endian; see wire-formats.md §10):
//!   state  = seq(8) ‖ count(4) ‖ count × [hash(20) ‖ score(8) ‖ nonce(8)]
//!   intent = hash(20) ‖ points(8) ‖ nonce(8) ‖ sig(65)          (101 bytes)
//!   batch  = n(2) ‖ n × intent
//!   intent message = blake2b_256(DOMAIN ‖ game_id ‖ hash ‖ points ‖ nonce)

#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use ckb_hash::blake2b_256;

// ---- Layout constants (shared with the type script + wire-formats.md) ------
pub const GAME_ID_LEN: usize = 32;
pub const SIGNATURE_LEN: usize = 65;
pub const ENTRY_LEN: usize = 36; // hash(20) ‖ score(8) ‖ nonce(8)
pub const INTENT_LEN: usize = 101; // hash(20) ‖ points(8) ‖ nonce(8) ‖ sig(65)
pub const STATE_HEADER_LEN: usize = 12; // seq(8) ‖ count(4)

/// Intent signing-message domain separator.
pub const INTENT_DOMAIN: &[u8] = b"ckb-controller/game-intent/v1";

#[derive(Debug, PartialEq, Eq)]
pub enum GameError {
    StateMalformed,
    BatchMalformed,
    NonceNotFresh,
    PointsTooHigh,
    Overflow,
}

// ═════════════════════════════ GAME RULE ════════════════════════════════════
// The demo rule is a scoreboard: a move is "player scores `points`", capped
// per move. Replace this section (and only this section) to build your game.

/// Per-move rule cap (the demo scoreboard rule).
pub const MAX_POINTS_PER_MOVE: u64 = 1000;

/// One player's standing on the shared board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerEntry {
    pub hash: [u8; 20],
    pub score: u64,
    pub nonce: u64,
}

/// The shared game state = the game cell's data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameState {
    pub seq: u64,
    pub players: Vec<PlayerEntry>,
}

impl GameState {
    /// Apply one intent to the board — THE game rule, enforced on-chain.
    ///
    /// The nonce discipline (strictly `prev + 1`; first move = 1) is the
    /// aggregator's anti-replay and should stay in any game. Does NOT verify
    /// the signature (the type script does, via ckb-auth) and does NOT bump
    /// `seq` (done once per batch by [`GameState::apply_batch`]).
    pub fn apply_intent(&mut self, intent: &Intent) -> Result<(), GameError> {
        if intent.points > MAX_POINTS_PER_MOVE {
            return Err(GameError::PointsTooHigh);
        }
        match self.players.iter_mut().find(|p| p.hash == intent.hash) {
            Some(p) => {
                if intent.nonce != p.nonce.checked_add(1).ok_or(GameError::Overflow)? {
                    return Err(GameError::NonceNotFresh);
                }
                p.score = p.score.checked_add(intent.points).ok_or(GameError::Overflow)?;
                p.nonce = intent.nonce;
            }
            None => {
                if intent.nonce != 1 {
                    return Err(GameError::NonceNotFresh);
                }
                self.players.push(PlayerEntry {
                    hash: intent.hash,
                    score: intent.points,
                    nonce: 1,
                });
            }
        }
        Ok(())
    }
}

// ═══════════════════════════ END GAME RULE ══════════════════════════════════

impl GameState {
    pub fn empty() -> Self {
        GameState::default()
    }

    /// Serialize to the game cell's `data`: seq ‖ count ‖ [hash ‖ score ‖ nonce]*.
    pub fn encode(&self) -> Vec<u8> {
        let mut d = Vec::with_capacity(STATE_HEADER_LEN + self.players.len() * ENTRY_LEN);
        d.extend_from_slice(&self.seq.to_le_bytes());
        d.extend_from_slice(&(self.players.len() as u32).to_le_bytes());
        for p in &self.players {
            d.extend_from_slice(&p.hash);
            d.extend_from_slice(&p.score.to_le_bytes());
            d.extend_from_slice(&p.nonce.to_le_bytes());
        }
        d
    }

    /// Parse a game cell's `data` (empty data = the empty state).
    pub fn decode(data: &[u8]) -> Result<Self, GameError> {
        if data.is_empty() {
            return Ok(GameState::empty());
        }
        if data.len() < STATE_HEADER_LEN {
            return Err(GameError::StateMalformed);
        }
        let seq = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let count = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
        if data.len() != STATE_HEADER_LEN + count * ENTRY_LEN {
            return Err(GameError::StateMalformed);
        }
        let mut players = Vec::with_capacity(count);
        for i in 0..count {
            let off = STATE_HEADER_LEN + i * ENTRY_LEN;
            let e = &data[off..off + ENTRY_LEN];
            players.push(PlayerEntry {
                hash: e[0..20].try_into().unwrap(),
                score: u64::from_le_bytes(e[20..28].try_into().unwrap()),
                nonce: u64::from_le_bytes(e[28..36].try_into().unwrap()),
            });
        }
        Ok(GameState { seq, players })
    }

    /// Apply a whole batch in order and bump `seq` by the number applied —
    /// exactly the transition the type script verifies.
    pub fn apply_batch(&mut self, intents: &[Intent]) -> Result<(), GameError> {
        for intent in intents {
            self.apply_intent(intent)?;
        }
        self.seq = self
            .seq
            .checked_add(intents.len() as u64)
            .ok_or(GameError::Overflow)?;
        Ok(())
    }

    /// Order-insensitive equality: the operator may serialise players in any
    /// order; only the (hash → score, nonce) mapping and `seq` are consensus.
    /// This is the comparison the TYPE SCRIPT uses on the output state.
    pub fn equals_unordered(&self, other: &GameState) -> bool {
        if self.seq != other.seq || self.players.len() != other.players.len() {
            return false;
        }
        self.players.iter().all(|p| {
            other
                .players
                .iter()
                .any(|q| q.hash == p.hash && q.score == p.score && q.nonce == p.nonce)
        })
    }
}

/// A single session-signed move.
#[derive(Debug, Clone)]
pub struct Intent {
    pub hash: [u8; 20],
    pub points: u64,
    pub nonce: u64,
    pub sig: [u8; SIGNATURE_LEN],
}

impl Intent {
    /// Serialize to the 101-byte on-wire intent.
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(INTENT_LEN);
        b.extend_from_slice(&self.hash);
        b.extend_from_slice(&self.points.to_le_bytes());
        b.extend_from_slice(&self.nonce.to_le_bytes());
        b.extend_from_slice(&self.sig);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, GameError> {
        if b.len() != INTENT_LEN {
            return Err(GameError::BatchMalformed);
        }
        Ok(Intent {
            hash: b[0..20].try_into().unwrap(),
            points: u64::from_le_bytes(b[20..28].try_into().unwrap()),
            nonce: u64::from_le_bytes(b[28..36].try_into().unwrap()),
            sig: b[36..101].try_into().unwrap(),
        })
    }
}

/// The message a player signs for an intent: blake2b_256(DOMAIN ‖ game_id ‖
/// hash ‖ points ‖ nonce). Bound to the game and this exact move — no
/// cross-game reuse, no replay (the nonce discipline rejects reuse).
pub fn intent_message(game_id: &[u8; 32], hash: &[u8; 20], points: u64, nonce: u64) -> [u8; 32] {
    let mut buf = Vec::with_capacity(INTENT_DOMAIN.len() + GAME_ID_LEN + 20 + 8 + 8);
    buf.extend_from_slice(INTENT_DOMAIN);
    buf.extend_from_slice(game_id);
    buf.extend_from_slice(hash);
    buf.extend_from_slice(&points.to_le_bytes());
    buf.extend_from_slice(&nonce.to_le_bytes());
    blake2b_256(&buf)
}

/// Frame a batch for the transition witness's `input_type`: n(2 LE) ‖ intents.
pub fn encode_batch(intents: &[Intent]) -> Vec<u8> {
    let mut b = Vec::with_capacity(2 + intents.len() * INTENT_LEN);
    b.extend_from_slice(&(intents.len() as u16).to_le_bytes());
    for i in intents {
        b.extend_from_slice(&i.encode());
    }
    b
}

/// Parse a batch (the type script's exact parsing).
pub fn decode_batch(batch: &[u8]) -> Result<Vec<Intent>, GameError> {
    if batch.len() < 2 {
        return Err(GameError::BatchMalformed);
    }
    let n = u16::from_le_bytes(batch[0..2].try_into().unwrap()) as usize;
    if batch.len() != 2 + n * INTENT_LEN {
        return Err(GameError::BatchMalformed);
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let off = 2 + i * INTENT_LEN;
        out.push(Intent::decode(&batch[off..off + INTENT_LEN])?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn intent(hash: [u8; 20], points: u64, nonce: u64) -> Intent {
        Intent { hash, points, nonce, sig: [0u8; SIGNATURE_LEN] }
    }

    #[test]
    fn state_roundtrip() {
        let mut s = GameState::empty();
        s.apply_batch(&[intent([1u8; 20], 10, 1), intent([2u8; 20], 20, 1)]).unwrap();
        let decoded = GameState::decode(&s.encode()).unwrap();
        assert_eq!(decoded, s);
        assert_eq!(decoded.seq, 2);
    }

    #[test]
    fn empty_and_zero_length_agree() {
        assert_eq!(GameState::decode(&[]).unwrap(), GameState::empty());
        assert_eq!(GameState::decode(&GameState::empty().encode()).unwrap(), GameState::empty());
    }

    #[test]
    fn apply_batch_bumps_seq_and_accumulates() {
        let mut s = GameState::empty();
        s.apply_batch(&[intent([1u8; 20], 10, 1)]).unwrap();
        s.apply_batch(&[intent([1u8; 20], 5, 2)]).unwrap();
        assert_eq!(s.seq, 2);
        assert_eq!(s.players[0].score, 15);
        assert_eq!(s.players[0].nonce, 2);
    }

    #[test]
    fn first_move_must_be_nonce_one() {
        let mut s = GameState::empty();
        assert_eq!(s.apply_intent(&intent([1u8; 20], 10, 2)), Err(GameError::NonceNotFresh));
    }

    #[test]
    fn rejects_stale_nonce() {
        let mut s = GameState::empty();
        s.apply_batch(&[intent([1u8; 20], 10, 1)]).unwrap();
        assert_eq!(s.apply_batch(&[intent([1u8; 20], 10, 1)]), Err(GameError::NonceNotFresh));
    }

    #[test]
    fn points_over_max_rejected() {
        let mut s = GameState::empty();
        assert_eq!(
            s.apply_intent(&intent([1u8; 20], MAX_POINTS_PER_MOVE + 1, 1)),
            Err(GameError::PointsTooHigh)
        );
    }

    #[test]
    fn batch_roundtrip() {
        let intents = vec![intent([9u8; 20], 100, 1), intent([8u8; 20], 200, 1)];
        let bytes = encode_batch(&intents);
        let back = decode_batch(&bytes).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[1].points, 200);
    }

    #[test]
    fn equals_is_order_insensitive() {
        let a = GameState {
            seq: 3,
            players: vec![
                PlayerEntry { hash: [1u8; 20], score: 10, nonce: 1 },
                PlayerEntry { hash: [2u8; 20], score: 20, nonce: 2 },
            ],
        };
        let b = GameState {
            seq: 3,
            players: vec![
                PlayerEntry { hash: [2u8; 20], score: 20, nonce: 2 },
                PlayerEntry { hash: [1u8; 20], score: 10, nonce: 1 },
            ],
        };
        assert!(a.equals_unordered(&b));
        assert!(!a.equals_unordered(&GameState { seq: 4, ..b.clone() }));
    }
}
