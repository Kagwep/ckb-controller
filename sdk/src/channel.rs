//! `ChannelSession` — the "every dev uses it easily" surface for the
//! session-funded payment channel: **`open` → `pay`×N → `close`.**
//!
//! This is the controller's twist on a Fiber channel. Two layers cooperate:
//!
//! - **L1 (this SDK):** the controller session *authorizes* the channel. `open`
//!   builds the funding transaction whose output is the Fiber funding-lock cell,
//!   bounded by the session's spend cap (= the channel budget) and allowed by the
//!   session's policy allowlist (the funding-lock). `close` builds the settle
//!   transaction that returns net funds to the account. Both are partial
//!   (unsigned) transactions, ready for the session key to sign last — exactly
//!   the same discipline the rest of the SDK uses.
//!
//! - **L2 (the [`FiberRail`] trait):** the actual off-chain payments. Each `pay`
//!   is an off-chain Fiber update — zero L1, no popup. The trait keeps the L2
//!   transport swappable: [`MockRail`] runs the whole loop in-memory today (no
//!   FNN required); a real Fiber node implements the same three methods later
//!   (roadmap step 3) without touching the L1 authorization above.
//!
//! The bracket is the whole point: **one L1 funding tx + N off-chain pays + one
//! L1 settle tx.** The funding amount is the session's entire economic exposure
//! (the on-chain spend cap enforces it), so a compromised session key can lose at
//! most the channel budget — never the account.

use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, Script},
    prelude::*,
};

use crate::{policy_leaf_for_script, proof_region, session_params, POLICY_KIND_LOCK};

// ---- L2 transport seam -----------------------------------------------------

/// A Fiber node peer (counterparty) — opaque to the controller. For Fiber this
/// is a node public key / connection id; the controller never interprets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerId(pub Vec<u8>);

impl From<&str> for PeerId {
    fn from(s: &str) -> Self {
        PeerId(s.as_bytes().to_vec())
    }
}

/// Net balances when a channel closes, in shannons. `local` lands back on the
/// account at settle; `remote` is what the counterparty earned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settlement {
    pub local: u128,
    pub remote: u128,
}

/// The off-chain payment rail. Implemented in-memory by [`MockRail`] for dev and
/// tests, and by a real Fiber FNN in production — the controller's L1
/// authorization (open/settle tx building) is identical either way.
pub trait FiberRail {
    /// Opaque channel handle returned by [`open`](FiberRail::open).
    type ChannelId: Clone;
    type Error: core::fmt::Debug;

    /// Register a channel with the counterparty, funded with `budget` shannons,
    /// backed by the on-chain funding cell at `funding`. Called by
    /// [`ChannelSession::open`] *after* the L1 funding tx is built (so `funding`
    /// is the real funding outpoint).
    fn open(
        &mut self,
        peer: &PeerId,
        budget: u128,
        funding: &OutPoint,
    ) -> Result<Self::ChannelId, Self::Error>;

    /// Send an off-chain micropayment of `amount` shannons. No L1.
    fn pay(&mut self, channel: &Self::ChannelId, amount: u128) -> Result<(), Self::Error>;

    /// Tear down the channel, returning the net [`Settlement`].
    fn close(&mut self, channel: &Self::ChannelId) -> Result<Settlement, Self::Error>;
}

// ---- In-memory rail (dev + tests) ------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockRailError {
    /// `pay` would push the local balance below zero (exceeds remaining budget).
    InsufficientBalance,
    /// Channel id was never opened on this rail.
    UnknownChannel,
}

struct MockChannel {
    local: u128,
    remote: u128,
}

/// An in-memory [`FiberRail`] that tracks per-channel balances and rejects
/// overspend. Lets a developer run the full `open → pay → close` loop with no
/// Fiber node. Channel ids are sequential `u64`s.
#[derive(Default)]
pub struct MockRail {
    channels: Vec<MockChannel>,
}

impl MockRail {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_mut(&mut self, id: u64) -> Result<&mut MockChannel, MockRailError> {
        self.channels
            .get_mut(id as usize)
            .ok_or(MockRailError::UnknownChannel)
    }
}

impl FiberRail for MockRail {
    type ChannelId = u64;
    type Error = MockRailError;

    fn open(
        &mut self,
        _peer: &PeerId,
        budget: u128,
        _funding: &OutPoint,
    ) -> Result<u64, MockRailError> {
        let id = self.channels.len() as u64;
        self.channels.push(MockChannel {
            local: budget,
            remote: 0,
        });
        Ok(id)
    }

    fn pay(&mut self, channel: &u64, amount: u128) -> Result<(), MockRailError> {
        let ch = self.get_mut(*channel)?;
        ch.local = ch
            .local
            .checked_sub(amount)
            .ok_or(MockRailError::InsufficientBalance)?;
        ch.remote += amount;
        Ok(())
    }

    fn close(&mut self, channel: &u64) -> Result<Settlement, MockRailError> {
        let ch = self.get_mut(*channel)?;
        Ok(Settlement {
            local: ch.local,
            remote: ch.remote,
        })
    }
}

// ---- L1 authorization: args + tx building ----------------------------------

/// Build the session params for a channel session: scoped to a single allowed
/// destination (the Fiber `funding_lock`) with `budget` shannons as the spend
/// cap. Feed the result to [`registered_args`](crate::registered_args).
///
/// Single-destination policy ⇒ the policies root is just the funding-lock's
/// policy leaf, and the per-output proof is empty (see
/// [`channel_proof_region`]).
pub fn channel_session_params(
    session_pubkey_hash: &[u8; 20],
    expires_at: u64,
    funding_lock: &Script,
    budget: u64,
    guardian_pubkey_hash: &[u8; 20],
) -> Vec<u8> {
    let root = policy_leaf_for_script(funding_lock); // single leaf ⇒ root == leaf
    session_params(
        session_pubkey_hash,
        expires_at,
        &root,
        budget as u128,
        guardian_pubkey_hash,
    )
}

/// The proof region for a channel funding tx: one LOCK-kind entry with an empty
/// proof, for the single funding-cell output (the account-change output goes
/// back to the account lock, which the lock allows without a proof).
pub fn channel_proof_region() -> Vec<u8> {
    proof_region(&[(POLICY_KIND_LOCK, Vec::new())])
}

/// Everything the L1 builders need about the live account cell and deps.
#[derive(Clone)]
pub struct ChannelConfig {
    /// The controller lock script protecting the account (output 0 is recreated
    /// under this same lock).
    pub account_lock: Script,
    /// The live account cell being spent.
    pub account_input: OutPoint,
    /// Its capacity, shannons.
    pub account_capacity: u64,
    /// The Fiber funding-lock — the one allowed channel destination.
    pub funding_lock: Script,
    /// `[lock_dep, auth_dep, …]`. Cleared from the signing message, so a
    /// paymaster may add fee deps freely.
    pub cell_deps: Vec<CellDep>,
    /// Header dep carrying the timestamp the lock checks expiry against.
    pub header_dep: Byte32,
}

/// A built, still-unsigned channel transaction plus the index of the cell the
/// caller cares about (funding cell on open, account cell on settle).
pub struct ChannelTx {
    /// Partial tx: session-sign [`tx_message`](crate::tx_message) and attach a
    /// [`session_witness_registered`](crate::session_witness_registered) witness
    /// (with [`channel_proof_region`] on open) before broadcasting.
    pub tx: TransactionView,
    /// Output index of the funding (open) / account (settle) cell.
    pub index: usize,
}

impl ChannelTx {
    /// The outpoint of [`index`](ChannelTx::index) in this tx. Stable before
    /// signing — witnesses are not covered by the tx hash.
    pub fn outpoint(&self) -> OutPoint {
        OutPoint::new(self.tx.hash(), self.index as u32)
    }
}

fn account_change(lock: Script, capacity: u64) -> CellOutput {
    CellOutput::new_builder()
        .capacity(Capacity::shannons(capacity).pack())
        .lock(lock)
        .build()
}

/// Build the L1 **funding** transaction: spend the account, emit `[account
/// change, funding cell(budget)]`. The session (scoped to `funding_lock`, spend
/// cap = budget) authorizes it. Caller signs + witnesses.
pub fn build_funding_tx(cfg: &ChannelConfig, budget: u64) -> ChannelTx {
    let change = cfg.account_capacity.saturating_sub(budget);
    let outputs = vec![
        account_change(cfg.account_lock.clone(), change),
        CellOutput::new_builder()
            .capacity(Capacity::shannons(budget).pack())
            .lock(cfg.funding_lock.clone())
            .build(),
    ];
    let tx = TransactionBuilder::default()
        .cell_deps(cfg.cell_deps.clone().pack())
        .header_dep(cfg.header_dep.clone())
        .input(CellInput::new_builder().previous_output(cfg.account_input.clone()).build())
        .outputs(outputs)
        .outputs_data([Bytes::new(), Bytes::new()].pack())
        .build();
    ChannelTx { tx, index: 1 }
}

/// Build the L1 **settle** transaction: spend `funding` (the funding cell) and
/// return `settlement.local` to the account lock. In production Fiber's
/// funding-lock co-signs this cooperative close; the SDK supplies the canonical
/// shape (output 0 back to the account).
pub fn build_settle_tx(
    cfg: &ChannelConfig,
    funding: OutPoint,
    settlement: Settlement,
) -> ChannelTx {
    let local = u64::try_from(settlement.local).unwrap_or(u64::MAX);
    let tx = TransactionBuilder::default()
        .cell_deps(cfg.cell_deps.clone().pack())
        .header_dep(cfg.header_dep.clone())
        .input(CellInput::new_builder().previous_output(funding).build())
        .output(account_change(cfg.account_lock.clone(), local))
        .output_data(Bytes::new().pack())
        .build();
    ChannelTx { tx, index: 0 }
}

// ---- Orchestrator ----------------------------------------------------------

#[derive(Debug)]
pub enum ChannelError<E> {
    /// `pay`/`close` called before `open`, or after `close`.
    NotOpen,
    /// `open` called on an already-open session.
    AlreadyOpen,
    /// A `pay` exceeding the remaining budget — rejected before hitting the rail.
    OverBudget {
        remaining: u128,
        requested: u128,
    },
    /// The underlying rail failed.
    Rail(E),
}

/// Ties the controller's L1 authorization to an L2 [`FiberRail`]. Drive it:
/// [`open`](ChannelSession::open) (→ broadcast the funding tx),
/// [`pay`](ChannelSession::pay) repeatedly (off-chain), then
/// [`close`](ChannelSession::close) (→ broadcast the settle tx).
pub struct ChannelSession<R: FiberRail> {
    cfg: ChannelConfig,
    rail: R,
    channel: Option<R::ChannelId>,
    funding: Option<OutPoint>,
    budget: u128,
    spent: u128,
}

impl<R: FiberRail> ChannelSession<R> {
    pub fn new(cfg: ChannelConfig, rail: R) -> Self {
        Self {
            cfg,
            rail,
            channel: None,
            funding: None,
            budget: 0,
            spent: 0,
        }
    }

    /// **Open (1 L1 tx).** Build the funding tx, register the channel on the
    /// rail against its real funding outpoint, and return the partial funding tx
    /// for the caller to session-sign and broadcast.
    pub fn open(
        &mut self,
        peer: &PeerId,
        budget: u64,
    ) -> Result<ChannelTx, ChannelError<R::Error>> {
        if self.channel.is_some() {
            return Err(ChannelError::AlreadyOpen);
        }
        let funding_tx = build_funding_tx(&self.cfg, budget);
        let funding = funding_tx.outpoint();
        let channel = self
            .rail
            .open(peer, budget as u128, &funding)
            .map_err(ChannelError::Rail)?;
        self.channel = Some(channel);
        self.funding = Some(funding);
        self.budget = budget as u128;
        self.spent = 0;
        Ok(funding_tx)
    }

    /// **Pay (0 L1).** An off-chain micropayment. Rejected client-side if it
    /// would exceed the remaining budget (the on-chain spend cap is the
    /// backstop, but we never want to ask the rail for an impossible payment).
    pub fn pay(&mut self, amount: u128) -> Result<(), ChannelError<R::Error>> {
        let channel = self.channel.as_ref().ok_or(ChannelError::NotOpen)?;
        let remaining = self.budget - self.spent;
        if amount > remaining {
            return Err(ChannelError::OverBudget {
                remaining,
                requested: amount,
            });
        }
        self.rail.pay(channel, amount).map_err(ChannelError::Rail)?;
        self.spent += amount;
        Ok(())
    }

    /// **Close (1 L1 tx).** Tear down the channel and build the settle tx that
    /// returns net funds to the account. Returns the [`Settlement`] and the
    /// partial settle tx. Consumes the session.
    pub fn close(mut self) -> Result<(Settlement, ChannelTx), ChannelError<R::Error>> {
        let channel = self.channel.take().ok_or(ChannelError::NotOpen)?;
        let funding = self.funding.take().ok_or(ChannelError::NotOpen)?;
        let settlement = self.rail.close(&channel).map_err(ChannelError::Rail)?;
        let settle_tx = build_settle_tx(&self.cfg, funding, settlement);
        Ok((settlement, settle_tx))
    }

    pub fn budget(&self) -> u128 {
        self.budget
    }
    pub fn spent(&self) -> u128 {
        self.spent
    }
    pub fn remaining(&self) -> u128 {
        self.budget - self.spent
    }
    pub fn is_open(&self) -> bool {
        self.channel.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registered_args;

    fn funding_lock() -> Script {
        Script::new_builder()
            .code_hash([0xABu8; 32].pack())
            .args(Bytes::from("fiber-funding-lock").pack())
            .build()
    }

    fn cfg() -> ChannelConfig {
        ChannelConfig {
            account_lock: Script::new_builder().args(Bytes::from("acct").pack()).build(),
            account_input: OutPoint::new([7u8; 32].pack(), 0),
            account_capacity: 1000,
            funding_lock: funding_lock(),
            cell_deps: Vec::new(),
            header_dep: [9u8; 32].pack(),
        }
    }

    #[test]
    fn full_loop_open_pay_close() {
        let mut s = ChannelSession::new(cfg(), MockRail::new());
        let open = s.open(&"game-node".into(), 500).unwrap();
        // funding cell is output 1, budget shannons.
        let cap: u64 = open.tx.output(1).unwrap().capacity().unpack();
        assert_eq!(cap, 500);
        // account change is output 0 = 1000 - 500.
        let change: u64 = open.tx.output(0).unwrap().capacity().unpack();
        assert_eq!(change, 500);
        assert!(s.is_open());

        for _ in 0..30 {
            s.pay(10).unwrap();
        }
        assert_eq!(s.spent(), 300);
        assert_eq!(s.remaining(), 200);

        let (settlement, _settle_tx) = s.close().unwrap();
        assert_eq!(settlement.local, 200);
        assert_eq!(settlement.remote, 300);
    }

    #[test]
    fn pay_over_budget_is_rejected_before_rail() {
        let mut s = ChannelSession::new(cfg(), MockRail::new());
        s.open(&"peer".into(), 100).unwrap();
        s.pay(80).unwrap();
        match s.pay(50) {
            Err(ChannelError::OverBudget { remaining, requested }) => {
                assert_eq!(remaining, 20);
                assert_eq!(requested, 50);
            }
            other => panic!("expected OverBudget, got {other:?}"),
        }
        // budget untouched by the rejected pay.
        assert_eq!(s.spent(), 80);
    }

    #[test]
    fn pay_before_open_is_not_open() {
        let mut s = ChannelSession::new(cfg(), MockRail::new());
        assert!(matches!(s.pay(1), Err(ChannelError::NotOpen)));
    }

    #[test]
    fn double_open_rejected() {
        let mut s = ChannelSession::new(cfg(), MockRail::new());
        s.open(&"peer".into(), 100).unwrap();
        assert!(matches!(s.open(&"peer".into(), 100), Err(ChannelError::AlreadyOpen)));
    }

    #[test]
    fn channel_session_args_have_funding_lock_root_and_budget_cap() {
        let params = channel_session_params(&[1u8; 20], 42, &funding_lock(), 500, &[0u8; 20]);
        // root (bytes 28..60) == policy leaf of the funding lock (single-leaf tree).
        assert_eq!(&params[28..60], &policy_leaf_for_script(&funding_lock()));
        // spend cap (bytes 60..76) == 500.
        let cap = u128::from_le_bytes(params[60..76].try_into().unwrap());
        assert_eq!(cap, 500);
        // full args are the registered length.
        assert_eq!(
            registered_args(&[2u8; 20], &params).len(),
            crate::REGISTERED_ARGS_LEN
        );
    }

    #[test]
    fn funding_outpoint_is_stable_before_signing() {
        let open = build_funding_tx(&cfg(), 500);
        let op = open.outpoint();
        let idx: u32 = op.index().unpack();
        assert_eq!(idx as usize, 1);
        // hash is the raw-tx hash, independent of any witness we'd add later.
        assert_eq!(op.tx_hash(), open.tx.hash());
    }
}
