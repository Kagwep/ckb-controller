//! Off-chain paymaster core for the controller session lock.
//!
//! Two security-critical, pure pieces (the rest — HTTP, CKB RPC, live-cell
//! selection, broadcast — is deployment glue and lives outside this crate):
//!
//! 1. [`authz`] — a capability gate. A relayer must not be an open relay; it
//!    sponsors only clients presenting a valid **sponsor token** (signed, scoped,
//!    expiring) behind the [`authz::Gate`] trait. Two implementations ship:
//!    [`authz::Ed25519Gate`], a minimal hand-rolled reference, and
//!    [`biscuit_gate::BiscuitGate`], the production gate over `biscuit-auth` —
//!    the same library (and version line) Fiber gates its RPC with, giving real
//!    datalog policies, offline attenuation, and revocation.
//!
//! 2. [`assemble`] — assemble-then-sign balancing. The paymaster appends its fee
//!    input + change to the client's partial tx *before* the client session-signs,
//!    so the session signature (over the cell_deps-cleared tx) covers the final
//!    inputs/outputs. (Same discipline as Fiber's `funding_tx`.)

use ckb_types::{
    core::TransactionView,
    packed::{CellDep, OutPoint, Script},
};

pub mod assemble;
pub mod authz;
pub mod biscuit_gate;

#[derive(Debug)]
pub enum PaymasterError {
    Unauthorized(authz::AuthzError),
    Assemble(assemble::AssembleError),
}

impl From<assemble::AssembleError> for PaymasterError {
    fn from(e: assemble::AssembleError) -> Self {
        PaymasterError::Assemble(e)
    }
}

/// A paymaster: gate first, then balance the client's partial transaction.
pub struct Paymaster<G: authz::Gate> {
    gate: G,
}

impl<G: authz::Gate> Paymaster<G> {
    pub fn new(gate: G) -> Self {
        Self { gate }
    }

    /// Verify the sponsor token, then append a fee input + change to `partial`.
    /// Returns the balanced (still unsigned) transaction for the client to
    /// session-sign last. `fee_request` describes the fee cell the relayer will
    /// contribute.
    #[allow(clippy::too_many_arguments)]
    pub fn sponsor(
        &self,
        token: &[u8],
        now: u64,
        required_scope: &str,
        partial: &TransactionView,
        fee_input: OutPoint,
        fee_input_capacity: u64,
        fee_shannons: u64,
        change_lock: Script,
        fee_cell_deps: &[CellDep],
    ) -> Result<TransactionView, PaymasterError> {
        self.gate
            .authorize(token, now, required_scope)
            .map_err(PaymasterError::Unauthorized)?;
        Ok(assemble::balance(
            partial,
            fee_input,
            fee_input_capacity,
            fee_shannons,
            change_lock,
            fee_cell_deps,
        )?)
    }
}
