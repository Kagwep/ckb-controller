//! The sponsored-relay orchestration: gate the token, size the fee, collect a
//! fee cell, balance the client's partial tx (assemble-then-sign), broadcast.
//!
//! This is the deployment glue the `controller-paymaster` core deliberately omits.
//! The cryptographic decisions still live in the core ([`Gate`] + [`balance`]);
//! this layer only resolves the live-chain inputs the core needs and moves bytes
//! over RPC.
//!
//! Note: the fee cell's own lock signature (e.g. secp256k1 for a real operator
//! wallet) is **not** produced here — that is the operator's key and the next
//! integration point. The session signature the *client* adds covers the account
//! input only (over the cell_deps-cleared raw tx, which excludes witnesses), so
//! the fee witness slot can be filled independently afterwards. Tests exercise
//! the flow with an always-success fee lock.

use ckb_types::{
    core::{Capacity, TransactionView},
    packed::{CellDep, OutPoint, Script},
    prelude::*,
};
use controller_paymaster::{
    assemble::{balance, min_fee, AssembleError},
    authz::{AuthzError, Gate},
};

use crate::rpc::CkbRpc;

#[derive(Debug)]
pub enum ServiceError {
    /// The sponsor token was rejected by the gate.
    Unauthorized(AuthzError),
    /// Balancing failed (the collected cell couldn't cover the fee).
    Assemble(AssembleError),
    /// The collector found no live cell able to cover fee + change.
    NoFeeCell,
    /// CKB RPC / network failure.
    Rpc(String),
    /// Malformed request (bad tx json, token, etc.).
    BadRequest(String),
}

impl From<AssembleError> for ServiceError {
    fn from(e: AssembleError) -> Self {
        ServiceError::Assemble(e)
    }
}

/// A sponsored (balanced, still client-unsigned) transaction plus the details a
/// client/operator needs to finish and audit it.
#[derive(Debug)]
pub struct Sponsored {
    /// Balanced tx: client's partial + the relayer's fee input and change output.
    pub tx: TransactionView,
    /// Fee paid, in shannons.
    pub fee: u64,
    /// The fee input the relayer contributed (its witness is left for the operator
    /// to sign with the fee lock's key).
    pub fee_input: OutPoint,
    /// Token subject, for the operator's rate-limiting / audit.
    pub subject: String,
}

/// Sponsored-relay service: a [`Gate`] over a [`CkbRpc`] node.
pub struct SponsorService<G: Gate, R: CkbRpc> {
    gate: G,
    rpc: R,
    /// The relayer's own lock: where fee cells are collected from and change goes.
    fee_lock: Script,
    /// Cell deps required to spend `fee_lock` (e.g. the secp256k1 dep group).
    fee_cell_deps: Vec<CellDep>,
    /// Fee rate, shannons per 1000 bytes.
    fee_rate: u64,
    /// The scope every sponsored token must carry.
    scope: String,
}

impl<G: Gate, R: CkbRpc> SponsorService<G, R> {
    pub fn new(
        gate: G,
        rpc: R,
        fee_lock: Script,
        fee_cell_deps: Vec<CellDep>,
        fee_rate: u64,
        scope: impl Into<String>,
    ) -> Self {
        Self {
            gate,
            rpc,
            fee_lock,
            fee_cell_deps,
            fee_rate,
            scope: scope.into(),
        }
    }

    /// Minimum capacity a change cell under `fee_lock` must hold (its occupied
    /// capacity with empty data), so the change output is itself valid.
    fn min_change(&self) -> u64 {
        ckb_types::packed::CellOutput::new_builder()
            .lock(self.fee_lock.clone())
            .build()
            .occupied_capacity(Capacity::zero())
            .expect("occupied capacity")
            .as_u64()
    }

    /// Gate the token, then collect + balance. `now` is unix seconds (injected so
    /// tests are deterministic).
    pub fn sponsor(
        &self,
        token: &[u8],
        partial: &TransactionView,
        now: u64,
    ) -> Result<Sponsored, ServiceError> {
        let cap = self
            .gate
            .authorize(token, now, &self.scope)
            .map_err(ServiceError::Unauthorized)?;

        // Size the fee against a dry-balanced tx (fee input + change included), so
        // the estimate reflects the bytes the relayer is about to add.
        let dry = balance(
            partial,
            OutPoint::new(Default::default(), 0),
            u64::MAX / 2,
            0,
            self.fee_lock.clone(),
            &self.fee_cell_deps,
        )?;
        let fee = min_fee(&dry, self.fee_rate).max(MIN_FEE_SHANNONS);

        // Need enough to cover the fee and leave a valid change cell.
        let needed = fee
            .checked_add(self.min_change())
            .ok_or(ServiceError::Assemble(AssembleError::InsufficientFee))?;
        let fee_cell = self
            .rpc
            .collect_fee_cell(&self.fee_lock, needed)
            .map_err(|e| ServiceError::Rpc(e.to_string()))?
            .ok_or(ServiceError::NoFeeCell)?;

        let tx = balance(
            partial,
            fee_cell.out_point.clone(),
            fee_cell.capacity,
            fee,
            self.fee_lock.clone(),
            &self.fee_cell_deps,
        )?;

        Ok(Sponsored {
            tx,
            fee,
            fee_input: fee_cell.out_point,
            subject: cap.subject,
        })
    }

    /// Broadcast a fully-signed (client session + fee lock) transaction.
    pub fn broadcast(
        &self,
        signed: &TransactionView,
    ) -> Result<ckb_types::packed::Byte32, ServiceError> {
        self.rpc
            .send_transaction(signed)
            .map_err(|e| ServiceError::Rpc(e.to_string()))
    }

    /// The hash of the node's tip header (for clients building an expiring session).
    pub fn tip_header_hash(&self) -> Result<ckb_types::packed::Byte32, ServiceError> {
        self.rpc
            .tip_header_hash()
            .map_err(|e| ServiceError::Rpc(e.to_string()))
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    pub fn fee_lock(&self) -> &Script {
        &self.fee_lock
    }
}

/// A floor so dust-sized txs still pay something the node won't reject.
pub const MIN_FEE_SHANNONS: u64 = 1000;
