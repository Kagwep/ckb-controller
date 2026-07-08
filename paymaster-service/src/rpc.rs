//! CKB JSON-RPC: the three calls the relayer needs — read the tip header, collect
//! a fee cell, broadcast. Behind the [`CkbRpc`] trait so [`crate::SponsorService`]
//! can be driven against a mock in tests (the same trait-seam pattern as the
//! `controller-paymaster` gate and the SDK's `FiberRail`).

use anyhow::{anyhow, Result};
use ckb_types::{
    core::TransactionView,
    packed::{Byte32, OutPoint, Script},
    prelude::*,
    H256,
};
use std::str::FromStr;

/// A live cell selected to pay a sponsored tx's fee.
#[derive(Debug, Clone)]
pub struct FeeCell {
    pub out_point: OutPoint,
    pub capacity: u64,
}

/// The CKB node interface the service depends on.
pub trait CkbRpc {
    /// Hash of the current tip header — used as the header dep an expiring session
    /// needs (a `NO_EXPIRY` session needs none).
    fn tip_header_hash(&self) -> Result<Byte32>;

    /// Find one live, *plain* cell (no type script, empty data) under `lock` with
    /// at least `min_capacity` shannons, suitable to consume as a fee source.
    /// `Ok(None)` means the collector found nothing usable (vs an RPC failure).
    fn collect_fee_cell(&self, lock: &Script, min_capacity: u64) -> Result<Option<FeeCell>>;

    /// Broadcast a fully-signed transaction; returns its hash.
    fn send_transaction(&self, tx: &TransactionView) -> Result<Byte32>;
}

/// `CkbRpc` over a CKB node's JSON-RPC (incl. its built-in indexer `get_cells`).
pub struct HttpCkbRpc {
    url: String,
}

impl HttpCkbRpc {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }

    fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let resp: serde_json::Value = ureq::post(&self.url)
            .send_json(serde_json::json!({
                "id": 1, "jsonrpc": "2.0", "method": method, "params": params
            }))?
            .into_json()?;
        if let Some(err) = resp.get("error") {
            if !err.is_null() {
                return Err(anyhow!("rpc {method} error: {err}"));
            }
        }
        Ok(resp.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }
}

fn json_script(s: &Script) -> serde_json::Value {
    serde_json::to_value(ckb_jsonrpc_types::Script::from(s.clone())).unwrap()
}

fn parse_hex_u64(v: &serde_json::Value) -> Result<u64> {
    let s = v.as_str().ok_or_else(|| anyhow!("expected hex string, got {v}"))?;
    Ok(u64::from_str_radix(s.trim_start_matches("0x"), 16)?)
}

impl HttpCkbRpc {
    /// Find the (single) live cell carrying `type_script`, with its data — how the
    /// game operator locates its tip cell on a real chain. A game cell is a
    /// singleton per game id, so the first match is the tip.
    pub fn find_cell_by_type(
        &self,
        type_script: &Script,
    ) -> Result<Option<(OutPoint, ckb_types::packed::CellOutput, Vec<u8>)>> {
        let search_key = serde_json::json!({
            "script": json_script(type_script),
            "script_type": "type",
            "with_data": true,
        });
        let res = self.call(
            "get_cells",
            serde_json::json!([search_key, "asc", "0x1", serde_json::Value::Null]),
        )?;
        let empty = vec![];
        let Some(obj) = res["objects"].as_array().unwrap_or(&empty).first() else {
            return Ok(None);
        };
        let output: ckb_jsonrpc_types::CellOutput =
            serde_json::from_value(obj["output"].clone())?;
        let tx_hash = obj["out_point"]["tx_hash"]
            .as_str()
            .ok_or_else(|| anyhow!("cell: no tx_hash"))?;
        let index = parse_hex_u64(&obj["out_point"]["index"])? as u32;
        let data_hex = obj["output_data"]
            .as_str()
            .ok_or_else(|| anyhow!("cell: no output_data (with_data requested)"))?;
        let data = hex::decode(data_hex.trim_start_matches("0x"))?;
        Ok(Some((
            OutPoint::new(H256::from_str(tx_hash.trim_start_matches("0x"))?.pack(), index),
            output.into(),
            data,
        )))
    }
}

impl CkbRpc for HttpCkbRpc {
    fn tip_header_hash(&self) -> Result<Byte32> {
        let tip = self.call("get_tip_header", serde_json::json!([]))?;
        let hash = tip["hash"]
            .as_str()
            .ok_or_else(|| anyhow!("get_tip_header: no hash"))?;
        Ok(H256::from_str(hash.trim_start_matches("0x"))?.pack())
    }

    fn collect_fee_cell(&self, lock: &Script, min_capacity: u64) -> Result<Option<FeeCell>> {
        // Restrict to plain cells: type script length 0, data length 0.
        let search_key = serde_json::json!({
            "script": json_script(lock),
            "script_type": "lock",
            "filter": {
                "script_len_range": ["0x0", "0x1"],
                "output_data_len_range": ["0x0", "0x1"],
            }
        });
        let res = self.call(
            "get_cells",
            serde_json::json!([search_key, "asc", "0x64", serde_json::Value::Null]),
        )?;
        let empty = vec![];
        for obj in res["objects"].as_array().unwrap_or(&empty) {
            if !obj["output"]["type"].is_null() {
                continue;
            }
            let capacity = parse_hex_u64(&obj["output"]["capacity"])?;
            if capacity < min_capacity {
                continue;
            }
            let tx_hash = obj["out_point"]["tx_hash"]
                .as_str()
                .ok_or_else(|| anyhow!("cell: no tx_hash"))?;
            let index = parse_hex_u64(&obj["out_point"]["index"])? as u32;
            return Ok(Some(FeeCell {
                out_point: OutPoint::new(H256::from_str(tx_hash.trim_start_matches("0x"))?.pack(), index),
                capacity,
            }));
        }
        Ok(None)
    }

    fn send_transaction(&self, tx: &TransactionView) -> Result<Byte32> {
        let jtx: ckb_jsonrpc_types::Transaction = tx.data().into();
        let hash = self.call(
            "send_transaction",
            serde_json::json!([serde_json::to_value(jtx)?, "passthrough"]),
        )?;
        let hash = hash
            .as_str()
            .ok_or_else(|| anyhow!("send_transaction: no hash returned"))?;
        Ok(H256::from_str(hash.trim_start_matches("0x"))?.pack())
    }
}
