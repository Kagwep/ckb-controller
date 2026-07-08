//! Deployment shell for `controller-paymaster`: a running sponsored-relay HTTP
//! service. The core crate is pure (gate + assemble-then-sign); this crate adds
//! the parts a real relayer needs and the core intentionally omits — CKB RPC,
//! live-cell collection, broadcast, and an HTTP front door.
//!
//! * [`rpc`] — [`CkbRpc`] trait + an HTTP implementation; a mock makes the
//!   orchestration testable off-chain.
//! * [`service`] — [`SponsorService`]: gate the biscuit token, size + collect a
//!   fee cell, balance the client's partial tx, broadcast.
//! * [`main`](../src/main.rs) — a `tiny_http` server exposing `/health`,
//!   `/sponsor`, `/broadcast`.

pub mod operator;
pub mod rpc;
pub mod service;
pub mod sighash;

pub use operator::{GameOperator, GameTip, OperatorError, Transition};
pub use rpc::{CkbRpc, FeeCell, HttpCkbRpc};
pub use sighash::sign_sighash_all;
pub use service::{ServiceError, Sponsored, SponsorService, MIN_FEE_SHANNONS};

use ckb_types::{core::TransactionView, prelude::*};

/// Current unix time in seconds (the `now` the gate checks token expiry against).
pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Convert RPC-shaped tx JSON into a packed [`TransactionView`].
pub fn tx_from_json(jtx: ckb_jsonrpc_types::Transaction) -> TransactionView {
    let packed: ckb_types::packed::Transaction = jtx.into();
    packed.into_view()
}

/// Convert a [`TransactionView`] into RPC-shaped tx JSON.
pub fn tx_to_json(tx: &TransactionView) -> ckb_jsonrpc_types::Transaction {
    tx.data().into()
}

// ---- HTTP request/response contract (shared by the server and clients) ----

#[derive(serde::Deserialize)]
pub struct SponsorRequest {
    /// base64 biscuit sponsor token.
    pub token: String,
    /// The client's partial (unsigned, no fee) transaction.
    pub partial_tx: ckb_jsonrpc_types::Transaction,
}

#[derive(serde::Serialize)]
pub struct SponsorResponse {
    /// Balanced tx for the client to session-sign last.
    pub balanced_tx: ckb_jsonrpc_types::Transaction,
    pub fee: ckb_jsonrpc_types::Uint64,
    /// The relayer's fee input, as `0x<txhash>:<index>`.
    pub fee_input: String,
    pub subject: String,
}

#[derive(serde::Deserialize)]
pub struct BroadcastRequest {
    pub signed_tx: ckb_jsonrpc_types::Transaction,
}

#[derive(serde::Serialize)]
pub struct BroadcastResponse {
    pub tx_hash: ckb_types::H256,
}

/// Render an `OutPoint` as the `0x<txhash>:<index>` form used in responses.
pub fn outpoint_str(op: &ckb_types::packed::OutPoint) -> String {
    let tx_hash = ckb_types::H256::from_slice(op.tx_hash().raw_data().as_ref()).unwrap();
    let index: u32 = op.index().unpack();
    format!("{tx_hash:#x}:{index}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::FeeCell;
    use ckb_types::{
        bytes::Bytes,
        core::TransactionBuilder,
        packed::{CellInput, CellOutput, OutPoint, Script},
    };
    use controller_paymaster::{
        authz::Capability,
        biscuit_gate::{BiscuitAuthority, BiscuitGate},
    };
    use std::cell::Cell;

    const SCOPE: &str = "ckb-controller-sponsor";
    const NOW: u64 = 1_700_000_000;
    const FAR_FUTURE: u64 = 2_000_000_000;
    const CKB: u64 = 100_000_000;

    /// A configurable mock node: hands out one fee cell (or none).
    struct MockRpc {
        fee_cell: Option<FeeCell>,
        sent: Cell<bool>,
    }
    impl CkbRpc for MockRpc {
        fn tip_header_hash(&self) -> anyhow::Result<ckb_types::packed::Byte32> {
            Ok(ckb_types::packed::Byte32::default())
        }
        fn collect_fee_cell(&self, _lock: &Script, min: u64) -> anyhow::Result<Option<FeeCell>> {
            Ok(self.fee_cell.clone().filter(|c| c.capacity >= min))
        }
        fn send_transaction(&self, tx: &TransactionView) -> anyhow::Result<ckb_types::packed::Byte32> {
            self.sent.set(true);
            Ok(tx.hash())
        }
    }

    fn fee_cell(cap: u64) -> FeeCell {
        FeeCell {
            out_point: OutPoint::new(ckb_types::packed::Byte32::default(), 7),
            capacity: cap,
        }
    }

    fn partial() -> TransactionView {
        // A bare client tx: one account input, one game output.
        TransactionBuilder::default()
            .input(
                CellInput::new_builder()
                    .previous_output(OutPoint::new(ckb_types::packed::Byte32::default(), 0))
                    .build(),
            )
            .output(
                CellOutput::new_builder()
                    .capacity((1000 * CKB).pack())
                    .lock(Script::default())
                    .build(),
            )
            .output_data(Bytes::new().pack())
            .build()
    }

    fn service(rpc: MockRpc) -> SponsorService<BiscuitGate, MockRpc> {
        let gate = BiscuitGate::new(
            &BiscuitAuthority::from_seed(&[7u8; 32]).unwrap().public_key(),
        )
        .unwrap();
        SponsorService::new(gate, rpc, Script::default(), vec![], 1000, SCOPE)
    }

    fn token(not_after: u64) -> String {
        BiscuitAuthority::from_seed(&[7u8; 32])
            .unwrap()
            .issue(&Capability {
                scope: SCOPE.into(),
                subject: "player-1".into(),
                not_after,
            })
            .unwrap()
    }

    #[test]
    fn sponsor_balances_authorized_request() {
        let svc = service(MockRpc {
            fee_cell: Some(fee_cell(1000 * CKB)),
            sent: Cell::new(false),
        });
        let out = svc
            .sponsor(token(FAR_FUTURE).as_bytes(), &partial(), NOW)
            .expect("authorized");

        // fee input appended, change output appended.
        assert_eq!(out.tx.inputs().len(), 2);
        assert_eq!(out.tx.outputs().len(), 2);
        assert_eq!(out.subject, "player-1");
        assert!(out.fee >= MIN_FEE_SHANNONS);
        // change = fee cell capacity - fee
        let change: u64 = out.tx.output(1).unwrap().capacity().unpack();
        assert_eq!(change, 1000 * CKB - out.fee);
    }

    #[test]
    fn sponsor_rejects_expired_token() {
        let svc = service(MockRpc {
            fee_cell: Some(fee_cell(1000 * CKB)),
            sent: Cell::new(false),
        });
        let err = svc
            .sponsor(token(NOW - 1).as_bytes(), &partial(), NOW)
            .expect_err("expired");
        assert!(matches!(err, ServiceError::Unauthorized(_)));
    }

    #[test]
    fn sponsor_reports_no_fee_cell() {
        // collector has only a too-small cell -> filtered out -> None.
        let svc = service(MockRpc {
            fee_cell: Some(fee_cell(10)),
            sent: Cell::new(false),
        });
        let err = svc
            .sponsor(token(FAR_FUTURE).as_bytes(), &partial(), NOW)
            .expect_err("no fee cell");
        assert!(matches!(err, ServiceError::NoFeeCell));
    }

    #[test]
    fn broadcast_forwards_to_node() {
        let svc = service(MockRpc {
            fee_cell: None,
            sent: Cell::new(false),
        });
        let hash = svc.broadcast(&partial()).unwrap();
        assert_eq!(hash, partial().hash());
    }
}
