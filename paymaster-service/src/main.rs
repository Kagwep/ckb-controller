//! Sponsored-relay HTTP server: the running front door for `controller-paymaster`.
//!
//! Endpoints (JSON over HTTP):
//!   GET  /health     -> { status, scope, fee_lock }
//!   POST /sponsor    -> { token, partial_tx }  =>  { balanced_tx, fee, fee_input, subject }
//!   POST /broadcast  -> { signed_tx }           =>  { tx_hash }
//!
//! Config (env):
//!   LISTEN          bind address           (default 127.0.0.1:9933)
//!   RPC             CKB JSON-RPC url        (default http://127.0.0.1:8114)
//!   BISCUIT_PUBKEY  authority key, ed25519/<hex>                    [required]
//!   SCOPE           required token scope    (default ckb-controller-sponsor)
//!   FEE_RATE        shannons per 1000 bytes (default 1000)
//!   FEE_PUBKEY_HASH 0x<20 bytes> — the relayer wallet's secp256k1 lock args [required]
//!   FEE_CELL_DEP    0x<txhash>:<idx> — secp256k1 dep group cell dep         [required]

use anyhow::{anyhow, Context, Result};
use ckb_types::{
    bytes::Bytes,
    core::{DepType, ScriptHashType},
    packed::{CellDep, OutPoint, Script},
    prelude::*,
    H256,
};
use controller_paymaster::biscuit_gate::BiscuitGate;
use paymaster_service::{
    now_unix_secs, outpoint_str, tx_from_json, tx_to_json, BroadcastRequest, BroadcastResponse,
    HttpCkbRpc, ServiceError, SponsorRequest, SponsorResponse, SponsorService,
};
use std::env;
use std::str::FromStr;
use tiny_http::{Method, Response, Server};

const SIGHASH_CODE_HASH: &str = "9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8";

fn env_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.into())
}

fn require(name: &str) -> Result<String> {
    env::var(name).map_err(|_| anyhow!("missing required env {name}"))
}

fn hex_bytes(s: &str) -> Result<Vec<u8>> {
    hex::decode(s.trim_start_matches("0x")).context("invalid hex")
}

fn outpoint_from_str(s: &str) -> Result<OutPoint> {
    let (tx, idx) = s.split_once(':').ok_or_else(|| anyhow!("want 0x<txhash>:<idx>"))?;
    Ok(OutPoint::new(
        H256::from_str(tx.trim_start_matches("0x"))?.pack(),
        idx.parse()?,
    ))
}

/// The relayer's own secp256k1_blake160 wallet lock (where fees come from / change
/// goes), from its pubkey hash.
fn fee_lock(pubkey_hash: &[u8]) -> Result<Script> {
    Ok(Script::new_builder()
        .code_hash(H256::from_str(SIGHASH_CODE_HASH)?.pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(pubkey_hash.to_vec()).pack())
        .build())
}

fn json_response(status: u16, body: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body.to_string())
        .with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap())
        .with_status_code(status)
}

fn status_for(err: &ServiceError) -> u16 {
    match err {
        ServiceError::Unauthorized(_) => 401,
        ServiceError::BadRequest(_) => 400,
        ServiceError::NoFeeCell => 503, // relayer temporarily out of fee cells
        ServiceError::Assemble(_) => 422,
        ServiceError::Rpc(_) => 502,
    }
}

fn err_json(err: &ServiceError) -> serde_json::Value {
    serde_json::json!({ "error": format!("{err:?}") })
}

fn handle_sponsor(
    svc: &SponsorService<BiscuitGate, HttpCkbRpc>,
    body: &str,
) -> (u16, serde_json::Value) {
    let req: SponsorRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return (400, serde_json::json!({ "error": format!("bad request: {e}") })),
    };
    let partial = tx_from_json(req.partial_tx);
    match svc.sponsor(req.token.as_bytes(), &partial, now_unix_secs()) {
        Ok(out) => {
            let resp = SponsorResponse {
                balanced_tx: tx_to_json(&out.tx),
                fee: out.fee.into(),
                fee_input: outpoint_str(&out.fee_input),
                subject: out.subject,
            };
            (200, serde_json::to_value(resp).unwrap())
        }
        Err(e) => (status_for(&e), err_json(&e)),
    }
}

fn handle_broadcast(
    svc: &SponsorService<BiscuitGate, HttpCkbRpc>,
    body: &str,
) -> (u16, serde_json::Value) {
    let req: BroadcastRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return (400, serde_json::json!({ "error": format!("bad request: {e}") })),
    };
    let signed = tx_from_json(req.signed_tx);
    match svc.broadcast(&signed) {
        Ok(hash) => {
            let resp = BroadcastResponse {
                tx_hash: H256::from_slice(hash.raw_data().as_ref()).unwrap(),
            };
            (200, serde_json::to_value(resp).unwrap())
        }
        Err(e) => (status_for(&e), err_json(&e)),
    }
}

fn main() -> Result<()> {
    let listen = env_or("LISTEN", "127.0.0.1:9933");
    let rpc_url = env_or("RPC", "http://127.0.0.1:8114");
    let scope = env_or("SCOPE", "ckb-controller-sponsor");
    let fee_rate: u64 = env_or("FEE_RATE", "1000").parse().context("FEE_RATE")?;

    let gate = BiscuitGate::new(&require("BISCUIT_PUBKEY")?)
        .map_err(|e| anyhow!("invalid BISCUIT_PUBKEY: {e:?}"))?;
    let fee_lock = fee_lock(&hex_bytes(&require("FEE_PUBKEY_HASH")?)?)?;
    let fee_dep = CellDep::new_builder()
        .out_point(outpoint_from_str(&require("FEE_CELL_DEP")?)?)
        .dep_type(DepType::DepGroup.into())
        .build();

    let svc = SponsorService::new(
        gate,
        HttpCkbRpc::new(rpc_url.clone()),
        fee_lock.clone(),
        vec![fee_dep],
        fee_rate,
        scope.clone(),
    );

    let server = Server::http(&listen).map_err(|e| anyhow!("bind {listen}: {e}"))?;
    println!("paymaster-service listening on http://{listen}");
    println!("  CKB RPC: {rpc_url}");
    println!("  scope:   {scope}");
    println!("  fee lock: {:#x}", fee_lock.calc_script_hash());

    for mut request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();
        let mut body = String::new();
        if request.as_reader().read_to_string(&mut body).is_err() {
            let _ = request.respond(json_response(400, serde_json::json!({ "error": "unreadable body" })));
            continue;
        }

        let (status, payload) = match (&method, url.as_str()) {
            (Method::Get, "/health") => (
                200,
                serde_json::json!({
                    "status": "ok",
                    "scope": svc.scope(),
                    "fee_lock": format!("{:#x}", svc.fee_lock().calc_script_hash()),
                }),
            ),
            (Method::Post, "/sponsor") => handle_sponsor(&svc, &body),
            (Method::Post, "/broadcast") => handle_broadcast(&svc, &body),
            _ => (404, serde_json::json!({ "error": "not found" })),
        };

        let _ = request.respond(json_response(status, payload));
    }
    Ok(())
}
