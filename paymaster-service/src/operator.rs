//! The **game aggregator operator** — the sequencing half of the multiplayer
//! controller (the CKB adaptation of Cartridge's shared game contract).
//!
//! On a cell chain a shared game-state cell is one live outpoint, so players can't
//! all write it concurrently. The operator resolves that: players submit
//! session-signed [`Intent`]s, the operator batches the pending ones into a single
//! transition tx that advances the game cell N -> N+1, and the on-chain type
//! script re-derives the same transition and rejects any deviation. The operator
//! therefore has **liveness power only** — it may order or censor, but every move
//! is signed and the result is verified on-chain, so it can neither forge a move
//! nor tamper a score.
//!
//! Trust seam mirrors [`crate::SponsorService`]: this builds the transition tx but
//! leaves the game cell's LOCK signature + fee to a `finalize` step (the operator's
//! own key / fee cell), because who may sequence is a lock decision, not a
//! type-script one. Tests use an always-success lock, so `finalize` is identity.

use ckb_types::{
    bytes::Bytes,
    core::{TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, Script, WitnessArgs},
    prelude::*,
};
use controller_sdk::game::{encode_batch, GameError, GameState, Intent};

use crate::rpc::CkbRpc;

#[derive(Debug)]
pub enum OperatorError {
    /// A queued intent doesn't apply to the current state (stale nonce, over-cap,
    /// overflow). The operator should drop it rather than build a doomed tx.
    Game(GameError),
    /// CKB RPC / broadcast failure.
    Rpc(String),
    /// The `finalize` step (fee + lock signature) failed.
    Finalize(String),
    /// Nothing to flush.
    Empty,
}

impl From<GameError> for OperatorError {
    fn from(e: GameError) -> Self {
        OperatorError::Game(e)
    }
}

/// The current on-chain game cell the operator is advancing (the "tip").
#[derive(Debug, Clone)]
pub struct GameTip {
    /// Live outpoint of the current game cell.
    pub out_point: OutPoint,
    /// Its `CellOutput` (capacity, lock, type) — reused for the next cell.
    pub output: CellOutput,
    /// Its decoded state (the cell data).
    pub state: GameState,
}

/// A built (but not-yet-finalized) transition: the tx that advances the game cell,
/// plus what the tip becomes once it's committed.
#[derive(Debug, Clone)]
pub struct Transition {
    /// The transition tx: input = current game cell, output 0 = next game cell,
    /// witness 0 carries the intent batch (input_type). Fee/lock still unsigned.
    pub tx: TransactionView,
    /// Number of intents applied.
    pub applied: usize,
    /// The next state (== output 0's data).
    pub next_state: GameState,
    /// The next game cell's `CellOutput` (index 0 of `tx`).
    pub next_output: CellOutput,
}

/// Batches session-signed intents into game-cell transitions.
pub struct GameOperator {
    /// The 32-byte game id (this game's type-script args) — binds intent messages.
    game_id: [u8; 32],
    /// Cell deps the transition needs: the game type-script code, ckb-auth, and the
    /// game cell's lock code (so both scripts can execute).
    cell_deps: Vec<CellDep>,
    /// The current game cell.
    tip: GameTip,
    /// Pending intents, in arrival order (the order they'll be applied).
    mempool: Vec<Intent>,
    /// Shannons each transition pays by shrinking the game cell (0 = free, e.g.
    /// mock mode / always-success tests). Self-funding keeps the tx single-input:
    /// no fee-cell selection, so the operator key's code cells are never at risk.
    fee: u64,
}

impl GameOperator {
    pub fn new(game_id: [u8; 32], cell_deps: Vec<CellDep>, tip: GameTip) -> Self {
        Self { game_id, cell_deps, tip, mempool: Vec::new(), fee: 0 }
    }

    /// Set the per-transition fee the game cell self-funds.
    pub fn with_fee(mut self, fee: u64) -> Self {
        self.fee = fee;
        self
    }

    pub fn game_id(&self) -> &[u8; 32] {
        &self.game_id
    }

    pub fn tip(&self) -> &GameTip {
        &self.tip
    }

    pub fn pending(&self) -> usize {
        self.mempool.len()
    }

    /// Queue a player's intent for the next transition. The intent is validated
    /// against the *projected* state (current tip + already-queued intents) and
    /// rejected here if it can't apply (stale nonce, over-cap, overflow) — so a
    /// doomed intent never enters the mempool and jams later flushes. Signatures
    /// are NOT checked here (the operator only sequences); a forged signature still
    /// queues and is rejected on-chain by the type script.
    pub fn submit(&mut self, intent: Intent) -> Result<(), OperatorError> {
        let mut projected = self.tip.state.clone();
        projected.apply_batch(&self.mempool)?; // replay the valid queue
        projected.apply_intent(&intent)?; // ...then the newcomer
        self.mempool.push(intent);
        Ok(())
    }

    /// Build the transition tx for all pending intents, without mutating the tip or
    /// draining the mempool (so a failed finalize/broadcast is retryable).
    pub fn build_transition(&self) -> Result<Transition, OperatorError> {
        if self.mempool.is_empty() {
            return Err(OperatorError::Empty);
        }

        // Re-derive the next state off-chain (same rule the type script checks).
        let mut next_state = self.tip.state.clone();
        next_state.apply_batch(&self.mempool)?;

        // Same lock + type; capacity shrinks by the fee (self-funded transition).
        let capacity: u64 = self.tip.output.capacity().unpack();
        let next_capacity = capacity.checked_sub(self.fee).ok_or_else(|| {
            OperatorError::Finalize(format!(
                "game cell capacity {capacity} cannot fund the {} fee",
                self.fee
            ))
        })?;
        let next_output = self
            .tip
            .output
            .clone()
            .as_builder()
            .capacity(next_capacity.pack())
            .build();
        let next_data = Bytes::from(next_state.encode());
        let batch = Bytes::from(encode_batch(&self.mempool));

        let witness = WitnessArgs::new_builder()
            .input_type(Some(batch).pack())
            .build()
            .as_bytes();

        let tx = TransactionBuilder::default()
            .cell_deps(self.cell_deps.clone())
            .input(
                CellInput::new_builder()
                    .previous_output(self.tip.out_point.clone())
                    .build(),
            )
            .output(next_output.clone())
            .output_data(next_data.pack())
            .witness(witness.pack())
            .build();

        Ok(Transition {
            tx,
            applied: self.mempool.len(),
            next_state,
            next_output,
        })
    }

    /// Build → finalize (fee + lock sig) → broadcast → advance the tip and drain
    /// the mempool. `finalize` turns the unsigned transition into a broadcastable
    /// tx (add the operator's fee cell + lock signature); it must NOT reorder or
    /// change output 0 (the game cell), or the tip advance below would be wrong.
    pub fn flush<R, F>(&mut self, rpc: &R, finalize: F) -> Result<Byte32, OperatorError>
    where
        R: CkbRpc + ?Sized,
        F: FnOnce(TransactionView) -> Result<TransactionView, String>,
    {
        let transition = self.build_transition()?;
        let signed = finalize(transition.tx).map_err(OperatorError::Finalize)?;

        let tx_hash = rpc
            .send_transaction(&signed)
            .map_err(|e| OperatorError::Rpc(e.to_string()))?;

        // The next game cell is output 0 of the just-broadcast tx.
        self.tip = GameTip {
            out_point: OutPoint::new(tx_hash.clone(), 0),
            output: transition.next_output,
            state: transition.next_state,
        };
        self.mempool.clear();
        Ok(tx_hash)
    }

    /// Drop all pending intents without advancing the tip. Used when a broadcast
    /// is rejected by the node (e.g. an intent with a forged signature — valid to
    /// the sequencer, invalid on-chain): keeping the batch would jam every later
    /// flush, so the operator sheds it and lets players resubmit.
    pub fn drop_pending(&mut self) {
        self.mempool.clear();
    }
}

/// Helper: assemble the game type script from its deployed code + a 32-byte game id.
pub fn game_type_script(code_hash: Byte32, hash_type: u8, game_id: &[u8; 32]) -> Script {
    Script::new_builder()
        .code_hash(code_hash)
        .hash_type(hash_type.into())
        .args(Bytes::from(game_id.to_vec()).pack())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::FeeCell;
    use controller_sdk::game::{intent_message, Intent};
    use std::cell::Cell;

    // A mock node that just records broadcasts and returns the tx hash.
    struct MockRpc {
        sent: Cell<u32>,
    }
    impl CkbRpc for MockRpc {
        fn tip_header_hash(&self) -> anyhow::Result<Byte32> {
            Ok(Byte32::default())
        }
        fn collect_fee_cell(&self, _lock: &Script, _min: u64) -> anyhow::Result<Option<FeeCell>> {
            Ok(None)
        }
        fn send_transaction(&self, tx: &TransactionView) -> anyhow::Result<Byte32> {
            self.sent.set(self.sent.get() + 1);
            Ok(tx.hash())
        }
    }

    fn tip_at(out_point: OutPoint, state: GameState) -> GameTip {
        GameTip {
            out_point,
            output: CellOutput::new_builder()
                .capacity(1000u64.pack())
                .lock(Script::default())
                .type_(Some(Script::default()).pack())
                .build(),
            state,
        }
    }

    // An unsigned intent (signature irrelevant to the operator's sequencing logic).
    fn intent(game_id: &[u8; 32], hash: [u8; 20], points: u64, nonce: u64) -> Intent {
        // compute the message so the shape matches a real one; sig left zeroed.
        let _ = intent_message(game_id, &hash, points, nonce);
        Intent { hash, points, nonce, sig: [0u8; 65] }
    }

    fn operator() -> GameOperator {
        let game_id = [5u8; 32];
        let tip = tip_at(OutPoint::new(Byte32::default(), 0), GameState::empty());
        GameOperator::new(game_id, vec![], tip)
    }

    #[test]
    fn build_transition_applies_and_bumps_seq() {
        let mut op = operator();
        let gid = *op.game_id();
        op.submit(intent(&gid, [1u8; 20], 10, 1)).unwrap();
        op.submit(intent(&gid, [2u8; 20], 20, 1)).unwrap();

        let t = op.build_transition().unwrap();
        assert_eq!(t.applied, 2);
        assert_eq!(t.next_state.seq, 2);
        assert_eq!(t.tx.outputs().len(), 1);
        // output 0's data == encoded next state.
        let data = t.tx.outputs_data().get(0).unwrap().raw_data();
        assert_eq!(data.as_ref(), t.next_state.encode().as_slice());
        // building doesn't drain the mempool.
        assert_eq!(op.pending(), 2);
    }

    #[test]
    fn empty_mempool_cannot_flush() {
        let mut op = operator();
        let rpc = MockRpc { sent: Cell::new(0) };
        assert!(matches!(
            op.flush(&rpc, Ok),
            Err(OperatorError::Empty)
        ));
        assert_eq!(rpc.sent.get(), 0);
    }

    #[test]
    fn stale_intent_rejected_at_submit() {
        let mut op = operator();
        let gid = *op.game_id();
        // first move for a new player must be nonce 1; nonce 2 is stale/invalid and
        // must be rejected at submit so it never jams the mempool.
        let err = op.submit(intent(&gid, [1u8; 20], 10, 2)).unwrap_err();
        assert!(matches!(err, OperatorError::Game(GameError::NonceNotFresh)));
        assert_eq!(op.pending(), 0);
    }

    #[test]
    fn bad_intent_does_not_poison_the_queue() {
        // a rejected intent must not block a later valid one (the mempool-jam bug).
        let mut op = operator();
        let gid = *op.game_id();
        assert!(op.submit(intent(&gid, [1u8; 20], 10, 2)).is_err()); // stale, rejected
        op.submit(intent(&gid, [2u8; 20], 20, 1)).unwrap(); // valid newcomer still ok
        assert_eq!(op.pending(), 1);
    }

    #[test]
    fn flush_broadcasts_and_advances_tip() {
        let mut op = operator();
        let gid = *op.game_id();
        op.submit(intent(&gid, [1u8; 20], 10, 1)).unwrap();
        let rpc = MockRpc { sent: Cell::new(0) };

        let hash = op.flush(&rpc, Ok).unwrap();
        assert_eq!(rpc.sent.get(), 1);
        // tip advanced: new outpoint = (hash, 0), state seq bumped, mempool drained.
        assert_eq!(op.tip().out_point.tx_hash(), hash);
        assert_eq!(op.tip().state.seq, 1);
        assert_eq!(op.tip().state.players[0].score, 10);
        assert_eq!(op.pending(), 0);

        // a second round accumulates on the advanced tip.
        op.submit(intent(&gid, [1u8; 20], 5, 2)).unwrap();
        op.flush(&rpc, Ok).unwrap();
        assert_eq!(op.tip().state.seq, 2);
        assert_eq!(op.tip().state.players[0].score, 15);
    }

    #[test]
    fn fee_shrinks_the_game_cell_each_transition() {
        let mut op = operator().with_fee(3);
        let gid = *op.game_id();
        let rpc = MockRpc { sent: Cell::new(0) };

        op.submit(intent(&gid, [1u8; 20], 10, 1)).unwrap();
        op.flush(&rpc, Ok).unwrap();
        let cap: u64 = op.tip().output.capacity().unpack();
        assert_eq!(cap, 997); // tip_at starts at 1000

        op.submit(intent(&gid, [1u8; 20], 5, 2)).unwrap();
        op.flush(&rpc, Ok).unwrap();
        let cap: u64 = op.tip().output.capacity().unpack();
        assert_eq!(cap, 994);
    }

    #[test]
    fn fee_larger_than_capacity_is_an_error() {
        let mut op = operator().with_fee(5000); // tip capacity is 1000
        let gid = *op.game_id();
        op.submit(intent(&gid, [1u8; 20], 10, 1)).unwrap();
        assert!(matches!(
            op.build_transition(),
            Err(OperatorError::Finalize(_))
        ));
    }

    #[test]
    fn failed_finalize_does_not_advance() {
        let mut op = operator();
        let gid = *op.game_id();
        op.submit(intent(&gid, [1u8; 20], 10, 1)).unwrap();
        let rpc = MockRpc { sent: Cell::new(0) };

        let err = op
            .flush(&rpc, |_tx| Err("no fee cell".to_string()))
            .unwrap_err();
        assert!(matches!(err, OperatorError::Finalize(_)));
        // nothing broadcast, tip unchanged, intent still queued (retryable).
        assert_eq!(rpc.sent.get(), 0);
        assert_eq!(op.tip().state.seq, 0);
        assert_eq!(op.pending(), 1);
    }
}
