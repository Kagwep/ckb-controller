//! End-to-end sanity for the deployment shell (`paymaster-service`): drive the
//! real `SponsorService` orchestration — biscuit gate, fee-cell collection
//! (mock node), assemble-then-balance — then have the client session-sign the
//! balanced tx and verify it against the REAL controller lock in CKB-VM.
//!
//! The CKB node is mocked (`MockRpc`) so this stays a pure off-chain test, but
//! everything between the HTTP boundary and the lock is the production code path:
//! the service gates the token, sizes the fee, and balances the tx; the lock then
//! accepts the session-signed result. (The fee cell uses an always-success lock,
//! so no fee-input signature is needed — the operator's fee key is out of scope,
//! exactly as in `paymaster_sanity`.)

use ckb_testtool::builtin::ALWAYS_SUCCESS;
use ckb_testtool::ckb_crypto::secp::{Generator, Privkey};
use ckb_testtool::ckb_hash::blake2b_256;
use ckb_testtool::ckb_types::{
    bytes::Bytes,
    core::{HeaderBuilder, TransactionBuilder, TransactionView},
    packed::*,
    prelude::*,
};
use ckb_testtool::context::Context;
use controller_paymaster::{
    authz::Capability,
    biscuit_gate::{BiscuitAuthority, BiscuitGate},
};
use controller_sdk as sdk;
use paymaster_service::{CkbRpc, FeeCell, ServiceError, SponsorService};
use std::fs;

const MAX_CYCLES: u64 = 100_000_000;
const FAR_FUTURE: u64 = 2_000_000_000;
const NOW: u64 = 1_700_000_000;
const SCOPE: &str = "ckb-controller-sponsor";

fn lock_binary() -> Bytes {
    fs::read("../build/release/controller-session-lock")
        .expect("build the lock first: ./build.sh")
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
fn sign(privkey: &Privkey, message: &[u8; 32]) -> Vec<u8> {
    privkey
        .sign_recoverable(&(*message).into())
        .expect("sign")
        .serialize()
}

/// A mock CKB node that hands back exactly the prepared fee cell.
struct MockRpc {
    fee_cell: Option<FeeCell>,
}
impl CkbRpc for MockRpc {
    fn tip_header_hash(&self) -> anyhow::Result<Byte32> {
        Ok(Byte32::default())
    }
    fn collect_fee_cell(&self, _lock: &Script, min: u64) -> anyhow::Result<Option<FeeCell>> {
        Ok(self.fee_cell.clone().filter(|c| c.capacity >= min))
    }
    fn send_transaction(&self, tx: &TransactionView) -> anyhow::Result<Byte32> {
        Ok(tx.hash())
    }
}

#[test]
fn service_sponsors_session_tx_against_real_lock() {
    let mut context = Context::default();
    let lock_op = context.deploy_cell(lock_binary());
    let auth_op = context.deploy_cell(auth_binary());
    let as_op = context.deploy_cell(ALWAYS_SUCCESS.clone());

    // biscuit authority + gate (the production gate).
    let authority = BiscuitAuthority::from_seed(&[7u8; 32]).unwrap();
    let token = authority
        .issue(&Capability {
            scope: SCOPE.into(),
            subject: "player-1".into(),
            not_after: FAR_FUTURE,
        })
        .unwrap();

    // controller account.
    let owner = Generator::new().gen_privkey();
    let session = Generator::new().gen_privkey();
    let params = sdk::session_params(
        &pubkey_hash(&session),
        FAR_FUTURE,
        &sdk::WILDCARD_ROOT,
        sdk::SPEND_CAP_UNLIMITED,
        &[0u8; 20],
    );
    let lock_script = context
        .build_script(&lock_op, sdk::registered_args(&pubkey_hash(&owner), &params))
        .expect("script");

    let header = HeaderBuilder::default()
        .timestamp(1_700_000_000_000u64.pack())
        .build();
    context.insert_header(header.clone());

    let account_input = context.create_cell(
        CellOutput::new_builder()
            .capacity(1000u64.pack())
            .lock(lock_script)
            .build(),
        Bytes::new(),
    );

    // client's partial tx (account input -> game output; no fee, unsigned).
    let partial = TransactionBuilder::default()
        .cell_dep(CellDep::new_builder().out_point(lock_op).build())
        .cell_dep(CellDep::new_builder().out_point(auth_op).build())
        .header_dep(header.hash())
        .input(CellInput::new_builder().previous_output(account_input).build())
        .output(
            CellOutput::new_builder()
                .capacity(1000u64.pack())
                .lock(Script::new_builder().args(Bytes::from("game").pack()).build())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    // the relayer's fee cell (always-success lock) + the dep to spend it.
    let always_success = context.build_script(&as_op, Bytes::new()).unwrap();
    let fee_cell = context.create_cell(
        CellOutput::new_builder()
            .capacity(100_000_000_000u64.pack()) // 1000 CKB, room for fee + change
            .lock(always_success.clone())
            .build(),
        Bytes::new(),
    );
    let fee_capacity: u64 = 100_000_000_000;
    let as_dep = CellDep::new_builder().out_point(as_op).build();

    // the service: real orchestration over a mock node.
    let svc = SponsorService::new(
        BiscuitGate::new(&authority.public_key()).unwrap(),
        MockRpc {
            fee_cell: Some(FeeCell {
                out_point: fee_cell,
                capacity: fee_capacity,
            }),
        },
        always_success, // fee lock = where change goes / fees come from
        vec![as_dep],
        1000, // fee rate, shannons/KB
        SCOPE,
    );

    let sponsored = svc
        .sponsor(token.as_bytes(), &partial, NOW)
        .expect("service should sponsor an authorized request");

    // account input (0) + fee input (1); game output (0) + change (1).
    assert_eq!(sponsored.tx.inputs().len(), 2);
    assert_eq!(sponsored.tx.outputs().len(), 2);
    assert_eq!(sponsored.subject, "player-1");

    // client session-signs the balanced tx LAST; the real lock verifies in-VM.
    let session_sig = sign(&session, &sdk::tx_message(&sponsored.tx));
    let account_witness = sdk::session_witness_registered(&session_sig, None, &[]);
    let signed = sponsored
        .tx
        .as_advanced_builder()
        .set_witnesses(vec![account_witness.pack(), Bytes::new().pack()])
        .build();

    let cycles = context
        .verify_tx(&signed, MAX_CYCLES)
        .expect("service-sponsored session tx should pass the real lock");
    println!("service_sponsors_session_tx_against_real_lock cycles: {cycles}");
}

#[test]
fn service_refuses_expired_token() {
    let authority = BiscuitAuthority::from_seed(&[7u8; 32]).unwrap();
    let token = authority
        .issue(&Capability {
            scope: SCOPE.into(),
            subject: "player-1".into(),
            not_after: NOW - 1, // expired at NOW
        })
        .unwrap();

    let svc = SponsorService::new(
        BiscuitGate::new(&authority.public_key()).unwrap(),
        MockRpc { fee_cell: None },
        Script::default(),
        vec![],
        1000,
        SCOPE,
    );

    let partial = TransactionBuilder::default().build();
    let err = svc
        .sponsor(token.as_bytes(), &partial, NOW)
        .expect_err("expired token must not be sponsored");
    assert!(matches!(err, ServiceError::Unauthorized(_)));
}
