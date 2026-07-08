//! Assemble-then-sign fee balancing.
//!
//! The client sends a *partial* transaction (its account input + intended
//! outputs, a placeholder/empty witness, and any header dep for expiry). The
//! paymaster appends ONE fee input (its own cell) and ONE change output, then
//! returns the balanced tx for the client to session-sign last. Because the
//! session signs `blake2b(raw tx with cell_deps cleared)`, the fee input/change
//! must be in place *before* signing — hence assemble-then-sign (cf. Fiber's
//! `funding_tx`: build with a placeholder, balance, sign last).
//!
//! This is append-only: the client's inputs, outputs, and witnesses are
//! preserved, so the account input stays at index 0 and its witness stays
//! witness[0] (where the controller lock reads it).

use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionView},
    packed::{CellDep, CellInput, CellOutput, OutPoint, Script},
    prelude::*,
};

#[derive(Debug, PartialEq, Eq)]
pub enum AssembleError {
    /// The fee cell can't cover the requested fee.
    InsufficientFee,
}

/// A conservative fee from the serialized tx size and a fee rate (shannons/KB).
/// The real relayer would size against the final tx; this is a helper for tests
/// and simple callers.
pub fn min_fee(tx: &TransactionView, fee_rate_shannons_per_kb: u64) -> u64 {
    let size = tx.data().as_slice().len() as u64;
    size.saturating_mul(fee_rate_shannons_per_kb).div_ceil(1000)
}

/// Append `fee_input` (+ its cell deps) and a change output back to the relayer's
/// `change_lock`, paying `fee_shannons`. Everything from `partial` is preserved.
pub fn balance(
    partial: &TransactionView,
    fee_input: OutPoint,
    fee_input_capacity: u64,
    fee_shannons: u64,
    change_lock: Script,
    fee_cell_deps: &[CellDep],
) -> Result<TransactionView, AssembleError> {
    let change = fee_input_capacity
        .checked_sub(fee_shannons)
        .ok_or(AssembleError::InsufficientFee)?;

    let change_output = CellOutput::new_builder()
        .capacity(Capacity::shannons(change).pack())
        .lock(change_lock)
        .build();

    let mut builder = partial
        .as_advanced_builder()
        .input(CellInput::new_builder().previous_output(fee_input).build())
        .output(change_output)
        .output_data(Bytes::new().pack());

    let existing: Vec<CellDep> = partial.cell_deps_iter().collect();
    for dep in fee_cell_deps {
        if !existing.contains(dep) {
            builder = builder.cell_dep(dep.clone());
        }
    }

    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::core::TransactionBuilder;

    #[test]
    fn balance_appends_fee_input_and_change() {
        let partial = TransactionBuilder::default()
            .output(
                CellOutput::new_builder()
                    .capacity(Capacity::shannons(1000).pack())
                    .lock(Script::default())
                    .build(),
            )
            .output_data(Bytes::new().pack())
            .build();

        let fee_input = OutPoint::new(Default::default(), 0);
        let balanced = balance(&partial, fee_input, 1000, 100, Script::default(), &[]).unwrap();

        assert_eq!(balanced.inputs().len(), 1); // fee input added
        assert_eq!(balanced.outputs().len(), 2); // original + change
        let change: u64 = balanced.output(1).unwrap().capacity().unpack();
        assert_eq!(change, 900); // 1000 - 100 fee
    }

    #[test]
    fn insufficient_fee_cell_rejected() {
        let partial = TransactionBuilder::default().build();
        let r = balance(&partial, OutPoint::new(Default::default(), 0), 50, 100, Script::default(), &[]);
        assert_eq!(r, Err(AssembleError::InsufficientFee));
    }
}
