//! Game aggregator operator — the runnable HTTP front door for [`GameOperator`].
//!
//! This is the sequencing service N players talk to: each posts a session-signed
//! intent, the operator batches it into a game-cell transition and advances the
//! shared board. It exposes the loop over HTTP (JSON, CORS-enabled so the browser
//! demo can call it directly):
//!
//!   GET  /health   -> { status, game_id, seq, pending, invoices }
//!   GET  /game     -> { seq, players: [ { hash, score, nonce } ] }
//!   POST /intent   -> { intent: "0x<101 bytes>" }  => { tx_hash, seq }   (submit + flush)
//!   POST /flush    -> {}                            => { tx_hash, seq }   (flush pending)
//!
//! ## Invoice relay (Phase 3 — non-custodial value rail)
//! The operator NEVER touches keys, preimages, or funds — it relays invoice
//! STRINGS between players so a payer never copy-pastes. Fibre TLCs move the value
//! player↔hub↔player; the operator only shuttles bytes and keeps a match log.
//!
//!   POST /invoice        { invoice: "fibt…", amount_ckb, to?, from?, game_id? }
//!                          => { id }              (payee publishes; bounded queue)
//!   GET  /invoice?for=<h> => { invoice: {id,invoice,amount_ckb,from,to,…} | null }
//!                          (payer polls; returns the next UNPAID invoice that is
//!                           open or addressed to <h> and not published by <h>)
//!   POST /invoice/paid   { id }  => { ok: true }  (payer/payee confirms settlement)
//!
//! `to`/`from` are player hashes (0x<20 bytes>); `to` omitted = open invoice
//! (anyone may pay). The queue is in-memory and bounded (`INVOICE_CAP`); the
//! durable record of who-paid-whom is the results log, not the queue.
//!
//! ## Results log (match history)
//! Every score / invoice-published / invoice-paid event is appended as one JSON
//! object per line to a JSONL file (`RESULTS_FILE`, default `game-results.jsonl`
//! in the cwd). No DB. The state-of-record for SCORES is the on-chain game cell;
//! this log is the operator's append-only match history.
//!
//!   GET  /results?n=<N>  => { results: [ …last N events… ] }   (default N = 50)
//!
//! ## Modes
//! Default is an **in-memory demo** (`CHAIN=mock`): the tip starts at an empty
//! genesis state and `flush` "broadcasts" to a mock node that just returns the tx
//! hash, so the whole multiplayer loop runs with no chain or funds — the analog of
//! the channel demo's `MockRail`. Intent SIGNATURES are only enforced on-chain, so
//! the demo accepts them without a live type script (exactly like mock mode
//! elsewhere).
//!
//! `CHAIN=http` is the live path: the operator locates the deployed game cell by
//! its type script (the tip), and each flush is finalized with the operator key's
//! standard sighash-all signature over the game cell's lock, then broadcast. The
//! transition self-funds its fee by shrinking the game cell (single-input tx —
//! the operator key's code cells are never touched). The tx shape is exactly what
//! `demo/game-advance.mjs` committed live.
//!
//! ## Config
//! Reads `controller.config.json` + `.controller/manifest.json` (repo root; the
//! path comes from `CONTROLLER_CONFIG`, default `controller.config.json` in the
//! cwd — so `cargo run -p paymaster-service --bin game-operator` from the repo
//! root is fully configured with NO env vars). Env vars override individual
//! values; built-in defaults apply when neither is present:
//!   LISTEN      bind address                  (config operator.listen | 127.0.0.1:9944)
//!   GAME_ID     0x<32 bytes> game id           (config gameId | 0x00…)
//!   CHAIN       mock | http                    (config operator.chain | mock)
//!   RPC         CKB JSON-RPC url (http mode)   (config networks[network].rpc | http://127.0.0.1:8114)
//!   KEYFILE     operator privkey hex file      (config keyFile; required for http)
//!   DEPLOY_FILE legacy game-deploy.json {codeHash,dep} (default: manifest game entry)
//!   AUTH_DEP    0x<txhash>:<index> ckb-auth    (manifest auth | the testnet deploy)
//!   SECP_DEP    0x<txhash>:<index> secp GROUP  (manifest secp256k1Sighash | testnet genesis)
//!   FEE         shannons per transition        (config operator.feeShannons | 100000)
//!   RESULTS_FILE match-history JSONL path        (default game-results.jsonl in cwd)

use anyhow::{anyhow, Context, Result};
use ckb_types::{
    core::{DepType, TransactionView},
    packed::{Byte32, CellDep, CellOutput, OutPoint, Script},
    prelude::*,
    H256,
};
use controller_sdk::game::{GameState, Intent};
use paymaster_service::{
    operator::game_type_script, sign_sighash_all, CkbRpc, FeeCell, GameOperator, GameTip,
    HttpCkbRpc, OperatorError,
};
use secp256k1::SecretKey;
use std::cell::Cell;
use std::env;
use std::str::FromStr;
use tiny_http::{Header, Method, Response, Server};

const CKB: u64 = 100_000_000;

/// Testnet ckb-auth code cell (the controller lock's deploy) — override via AUTH_DEP.
const DEFAULT_AUTH_DEP: &str =
    "0x539e202c058680b1945352800ad8d6edaaf2ec2034d6b2d575aad423bf1a401c:0";
/// Testnet secp256k1_blake160 dep GROUP (genesis) — override via SECP_DEP.
const DEFAULT_SECP_DEP: &str =
    "0xf8de3bb47d055cdf460d93a2a6e1b05f7432f9777c8c474abf4eec1d4aee5d37:0";

fn env_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.into())
}

fn hex_bytes(s: &str) -> Result<Vec<u8>> {
    hex::decode(s.trim_start_matches("0x")).map_err(|e| anyhow!("invalid hex: {e}"))
}

// --- broadcast backends -----------------------------------------------------

/// In-memory node: returns each tx's real hash without touching a chain, so the
/// operator's tip advances deterministically (the demo analog of MockRail).
struct MockChain {
    sent: Cell<u32>,
}
impl CkbRpc for MockChain {
    fn tip_header_hash(&self) -> Result<Byte32> {
        Ok(Byte32::default())
    }
    fn collect_fee_cell(&self, _lock: &Script, _min: u64) -> Result<Option<FeeCell>> {
        Ok(None)
    }
    fn send_transaction(&self, tx: &TransactionView) -> Result<Byte32> {
        self.sent.set(self.sent.get() + 1);
        Ok(tx.hash())
    }
}

// --- invoice relay + results log --------------------------------------------

/// Bounded per-server invoice queue: the operator holds at most this many relayed
/// invoice strings in memory. The durable record is the results log, not the queue.
const INVOICE_CAP: usize = 256;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One relayed invoice. The operator only ever sees the STRING (`invoice`) plus
/// routing metadata — never a key or preimage. `to`/`from` are player hashes;
/// `to == None` is an open invoice anyone may pay.
#[derive(Clone)]
struct RelayInvoice {
    id: u64,
    game_id: Option<String>,
    to: Option<String>,
    from: Option<String>,
    invoice: String,
    amount_ckb: u64,
    paid: bool,
    ts: u64,
}

impl RelayInvoice {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "game_id": self.game_id,
            "to": self.to,
            "from": self.from,
            "invoice": self.invoice,
            "amount_ckb": self.amount_ckb,
            "paid": self.paid,
            "ts": self.ts,
        })
    }
}

/// In-memory relay: a bounded FIFO of invoice strings the operator shuttles
/// between players. No custody — strings only.
struct Relay {
    invoices: Vec<RelayInvoice>,
    next_id: u64,
}

impl Relay {
    fn new() -> Self {
        Self { invoices: Vec::new(), next_id: 1 }
    }

    fn publish(
        &mut self,
        game_id: Option<String>,
        to: Option<String>,
        from: Option<String>,
        invoice: String,
        amount_ckb: u64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.invoices.push(RelayInvoice {
            id,
            game_id,
            to,
            from,
            invoice,
            amount_ckb,
            paid: false,
            ts: now_secs(),
        });
        // Bounded queue: shed the oldest once over cap (its settlement, if any, is
        // already in the results log).
        while self.invoices.len() > INVOICE_CAP {
            self.invoices.remove(0);
        }
        id
    }

    /// The next unpaid invoice a `payer` may pay: open or addressed to them, and
    /// not one they published themselves. With `payer == None` (mock single-page
    /// self-pay), the oldest unpaid invoice regardless of addressing.
    fn next_for(&self, payer: Option<&str>) -> Option<&RelayInvoice> {
        self.invoices.iter().find(|inv| {
            if inv.paid {
                return false;
            }
            if let Some(p) = payer {
                if inv.from.as_deref() == Some(p) {
                    return false; // don't hand a player their own invoice
                }
                if let Some(to) = &inv.to {
                    if to != p {
                        return false; // addressed to someone else
                    }
                }
            }
            true
        })
    }

    fn mark_paid(&mut self, id: u64) -> Option<RelayInvoice> {
        let inv = self.invoices.iter_mut().find(|i| i.id == id)?;
        inv.paid = true;
        Some(inv.clone())
    }
}

/// Append-only match-history log (one JSON object per line). Best-effort: a write
/// failure is logged to stderr but never fails a request (the on-chain cell, not
/// this file, is the state-of-record for scores).
struct ResultsLog {
    path: std::path::PathBuf,
}

impl ResultsLog {
    fn append(&self, event: serde_json::Value) {
        use std::io::Write;
        match std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            Ok(mut f) => {
                if let Err(e) = writeln!(f, "{event}") {
                    eprintln!("results log write failed: {e}");
                }
            }
            Err(e) => eprintln!("results log open failed ({}): {e}", self.path.display()),
        }
    }

    fn last(&self, n: usize) -> Vec<serde_json::Value> {
        let content = std::fs::read_to_string(&self.path).unwrap_or_default();
        let mut lines: Vec<serde_json::Value> =
            content.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let len = lines.len();
        if len > n {
            lines.split_off(len - n)
        } else {
            lines
        }
    }
}

/// Extract a query-string parameter (`?a=1&b=2`) from a request url. Values are
/// hex hashes / integers here, so no percent-decoding is needed.
fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then(|| v.to_string())
    })
}

/// POST /invoice — a payee publishes an invoice string for the relay to hold.
fn handle_publish_invoice(
    relay: &mut Relay,
    results: &ResultsLog,
    body: &str,
) -> (u16, serde_json::Value) {
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return (400, serde_json::json!({ "error": format!("bad request: {e}") })),
    };
    let invoice = match req.get("invoice").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return (400, serde_json::json!({ "error": "missing 'invoice' string field" })),
    };
    let amount_ckb = req.get("amount_ckb").and_then(|v| v.as_u64()).unwrap_or(0);
    let to = req.get("to").and_then(|v| v.as_str()).map(String::from);
    let from = req.get("from").and_then(|v| v.as_str()).map(String::from);
    let game_id = req.get("game_id").and_then(|v| v.as_str()).map(String::from);

    let id = relay.publish(game_id.clone(), to.clone(), from.clone(), invoice, amount_ckb);
    results.append(serde_json::json!({
        "ts": now_secs(), "kind": "invoice_published",
        "id": id, "from": from, "to": to, "amount_ckb": amount_ckb, "game_id": game_id,
    }));
    (200, serde_json::json!({ "id": id }))
}

/// POST /invoice/paid — settlement confirmation; records it against the match log.
fn handle_mark_paid(
    relay: &mut Relay,
    results: &ResultsLog,
    seq: u64,
    body: &str,
) -> (u16, serde_json::Value) {
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return (400, serde_json::json!({ "error": format!("bad request: {e}") })),
    };
    let id = match req.get("id").and_then(|v| v.as_u64()) {
        Some(id) => id,
        None => return (400, serde_json::json!({ "error": "missing numeric 'id' field" })),
    };
    match relay.mark_paid(id) {
        Some(inv) => {
            results.append(serde_json::json!({
                "ts": now_secs(), "kind": "invoice_paid",
                "id": inv.id, "from": inv.from, "to": inv.to,
                "amount_ckb": inv.amount_ckb, "seq": seq,
            }));
            (200, serde_json::json!({ "ok": true }))
        }
        None => (404, serde_json::json!({ "error": "unknown invoice id" })),
    }
}

// --- JSON rendering ---------------------------------------------------------

fn state_json(state: &GameState) -> serde_json::Value {
    let players: Vec<serde_json::Value> = state
        .players
        .iter()
        .map(|p| {
            serde_json::json!({
                "hash": format!("0x{}", hex::encode(p.hash)),
                "score": p.score,
                "nonce": p.nonce,
            })
        })
        .collect();
    serde_json::json!({ "seq": state.seq, "players": players })
}

/// Response with permissive CORS so the browser demo (different origin/port) works.
fn json_response(status: u16, body: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body.to_string())
        .with_header("Content-Type: application/json".parse::<Header>().unwrap())
        .with_header("Access-Control-Allow-Origin: *".parse::<Header>().unwrap())
        .with_header("Access-Control-Allow-Methods: GET, POST, OPTIONS".parse::<Header>().unwrap())
        .with_header("Access-Control-Allow-Headers: Content-Type".parse::<Header>().unwrap())
        .with_status_code(status)
}

fn op_error_status(e: &OperatorError) -> u16 {
    match e {
        OperatorError::Game(_) => 422,   // intent doesn't apply (stale nonce, over cap)
        OperatorError::Empty => 400,     // nothing to flush
        OperatorError::Finalize(_) => 500,
        OperatorError::Rpc(_) => 502,
    }
}

/// The finalize step turning a built transition into a broadcastable tx
/// (identity in mock mode; the operator's sighash-all signature in http mode).
type Finalize = Box<dyn Fn(TransactionView) -> Result<TransactionView, String>>;

/// Flush pending intents; on a node rejection (`Rpc`), shed the queue — a
/// forged-signature intent passes the sequencer (sigs are checked on-chain only)
/// and would otherwise jam every later flush. Players simply resubmit.
fn flush_or_shed(
    op: &mut GameOperator,
    rpc: &dyn CkbRpc,
    finalize: &Finalize,
) -> Result<Byte32, OperatorError> {
    let res = op.flush(rpc, |tx| finalize(tx));
    if let Err(OperatorError::Rpc(_)) = &res {
        op.drop_pending();
    }
    res
}

/// Submit an intent then flush it into a transition. Returns (status, body).
fn handle_intent(
    op: &mut GameOperator,
    rpc: &dyn CkbRpc,
    finalize: &Finalize,
    results: &ResultsLog,
    body: &str,
) -> (u16, serde_json::Value) {
    let req: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return (400, serde_json::json!({ "error": format!("bad request: {e}") })),
    };
    let intent_hex = match req.get("intent").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return (400, serde_json::json!({ "error": "missing 'intent' hex field" })),
    };
    let bytes = match hex_bytes(intent_hex) {
        Ok(b) => b,
        Err(e) => return (400, serde_json::json!({ "error": e.to_string() })),
    };
    let intent = match Intent::decode(&bytes) {
        Ok(i) => i,
        Err(e) => return (400, serde_json::json!({ "error": format!("bad intent: {e:?}") })),
    };

    // Keep the intent's fields for the match log (submit consumes the value).
    let (player, points) = (format!("0x{}", hex::encode(intent.hash)), intent.points);
    // Admission-validate the intent (stale nonce / over-cap rejected here, before
    // it can jam the queue). Signatures are enforced on-chain, not here.
    if let Err(e) = op.submit(intent) {
        return (op_error_status(&e), serde_json::json!({ "error": format!("{e:?}") }));
    }
    // Auto-flush this move into its own transition (the demo shows one step per
    // move; batching several submits before a single /flush is also supported).
    match flush_or_shed(op, rpc, finalize) {
        Ok(hash) => {
            let seq = op.tip().state.seq;
            results.append(serde_json::json!({
                "ts": now_secs(), "kind": "score",
                "player": player, "points": points, "seq": seq,
                "tx_hash": format!("{:#x}", hash),
            }));
            (200, serde_json::json!({ "tx_hash": format!("{:#x}", hash), "seq": seq }))
        }
        Err(e) => (op_error_status(&e), serde_json::json!({ "error": format!("{e:?}") })),
    }
}

fn handle_flush(
    op: &mut GameOperator,
    rpc: &dyn CkbRpc,
    finalize: &Finalize,
) -> (u16, serde_json::Value) {
    match flush_or_shed(op, rpc, finalize) {
        Ok(hash) => (
            200,
            serde_json::json!({ "tx_hash": format!("{:#x}", hash), "seq": op.tip().state.seq }),
        ),
        Err(e) => (op_error_status(&e), serde_json::json!({ "error": format!("{e:?}") })),
    }
}

// --- http-mode config helpers -------------------------------------------------

/// Parse `0x<txhash>:<index>` into an outpoint (index decimal or 0x-hex).
fn parse_outpoint(s: &str) -> Result<OutPoint> {
    let (hash, index) = s
        .split_once(':')
        .ok_or_else(|| anyhow!("expected 0x<txhash>:<index>, got {s}"))?;
    let index = if let Some(h) = index.strip_prefix("0x") {
        u32::from_str_radix(h, 16)?
    } else {
        index.parse()?
    };
    Ok(OutPoint::new(
        H256::from_str(hash.trim_start_matches("0x"))?.pack(),
        index,
    ))
}

fn dep(out_point: OutPoint, dep_type: DepType) -> CellDep {
    CellDep::new_builder()
        .out_point(out_point)
        .dep_type(dep_type.into())
        .build()
}

/// Parse a JSON file, or Null when absent/unreadable (all fallbacks then apply).
fn load_json(path: &std::path::Path) -> serde_json::Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// env var > config value > built-in default.
fn pick(env_name: &str, cfg: &serde_json::Value, default: &str) -> String {
    env::var(env_name)
        .ok()
        .or_else(|| cfg.as_str().map(String::from))
        .or_else(|| cfg.as_u64().map(|n| n.to_string()))
        .unwrap_or_else(|| default.into())
}

/// A manifest artifact's dep as `0x<txhash>:<index>`, if present.
fn manifest_dep(art: &serde_json::Value) -> Option<String> {
    Some(format!(
        "{}:{}",
        art["dep"]["txHash"].as_str()?,
        art["dep"]["index"].as_str().unwrap_or("0x0"),
    ))
}

fn main() -> Result<()> {
    // controller.config.json + .controller/manifest.json (env overrides values).
    let cfg_path = std::path::PathBuf::from(env_or("CONTROLLER_CONFIG", "controller.config.json"));
    let cfg = load_json(&cfg_path);
    let root = cfg_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let manifest = load_json(&root.join(".controller").join("manifest.json"));
    let network = env::var("NETWORK")
        .ok()
        .or_else(|| cfg["network"].as_str().map(String::from))
        .unwrap_or_else(|| "testnet".into());
    let net = &manifest[&network];

    let listen = pick("LISTEN", &cfg["operator"]["listen"], "127.0.0.1:9944");
    let game_id_bytes = hex_bytes(&pick("GAME_ID", &cfg["gameId"], &format!("0x{}", "00".repeat(32))))?;
    let game_id: [u8; 32] = game_id_bytes
        .try_into()
        .map_err(|_| anyhow!("GAME_ID must be 32 bytes"))?;
    let chain = pick("CHAIN", &cfg["operator"]["chain"], "mock");

    let (mut operator, rpc, finalize): (GameOperator, Box<dyn CkbRpc>, Finalize) = match chain
        .as_str()
    {
        "mock" => {
            // In-memory tip: a genesis (empty-state) game cell. The type script is a
            // placeholder here (no on-chain verification in mock mode); it carries the
            // real game_id so intent messages line up with a future live deployment.
            let type_script = game_type_script(Byte32::default(), 0x04 /* data2 */, &game_id);
            let tip_output = CellOutput::new_builder()
                .capacity((1000 * CKB).pack())
                .lock(Script::default())
                .type_(Some(type_script).pack())
                .build();
            let tip = GameTip {
                out_point: OutPoint::new(Byte32::default(), 0),
                output: tip_output,
                state: GameState::empty(),
            };
            (
                GameOperator::new(game_id, Vec::new(), tip),
                Box::new(MockChain { sent: Cell::new(0) }),
                Box::new(Ok),
            )
        }
        "http" => {
            let rpc_url = pick("RPC", &cfg["networks"][&network]["rpc"], "http://127.0.0.1:8114");
            let keyfile = env::var("KEYFILE")
                .ok()
                .or_else(|| cfg["keyFile"].as_str().map(String::from))
                .context("CHAIN=http needs KEYFILE (env or config keyFile)")?;
            let fee: u64 = pick("FEE", &cfg["operator"]["feeShannons"], "100000")
                .parse()
                .context("FEE")?;

            let sk_hex = std::fs::read_to_string(&keyfile).context("read KEYFILE")?;
            let sk = SecretKey::from_slice(&hex_bytes(sk_hex.trim())?)
                .map_err(|e| anyhow!("KEYFILE: bad secp256k1 key: {e}"))?;

            // The game code cell: the manifest's `game` entry, unless a legacy
            // DEPLOY_FILE (game-deploy.json from game-deploy.mjs) overrides it —
            // both are { codeHash, dep: { txHash, index } }.
            let deploy: serde_json::Value = match env::var("DEPLOY_FILE") {
                Ok(f) => serde_json::from_str(&std::fs::read_to_string(&f).context("read DEPLOY_FILE")?)?,
                Err(_) => net["game"].clone(),
            };
            let code_hash = H256::from_str(
                deploy["codeHash"]
                    .as_str()
                    .ok_or_else(|| anyhow!("no game codeHash (manifest `game` entry or DEPLOY_FILE)"))?
                    .trim_start_matches("0x"),
            )?
            .pack();
            let game_dep = parse_outpoint(
                &manifest_dep(&deploy).ok_or_else(|| anyhow!("no game dep (manifest or DEPLOY_FILE)"))?,
            )?;
            let auth_dep = parse_outpoint(&pick(
                "AUTH_DEP",
                &serde_json::Value::from(manifest_dep(&net["auth"])),
                DEFAULT_AUTH_DEP,
            ))?;
            let secp_dep = parse_outpoint(&pick(
                "SECP_DEP",
                &serde_json::Value::from(manifest_dep(&net["secp256k1Sighash"])),
                DEFAULT_SECP_DEP,
            ))?;

            // The live tip: the singleton cell carrying this game's type script.
            let rpc = HttpCkbRpc::new(rpc_url.clone());
            let type_script = game_type_script(code_hash, 0x04 /* data2 */, &game_id);
            let (out_point, output, data) = rpc
                .find_cell_by_type(&type_script)?
                .ok_or_else(|| anyhow!("no live game cell for this game id — run game-genesis.mjs"))?;
            let state = GameState::decode(&data)
                .map_err(|e| anyhow!("game cell data doesn't decode: {e:?}"))?;
            println!(
                "live tip: {} seq {}, {} player(s), {} CKB",
                paymaster_service::outpoint_str(&out_point),
                state.seq,
                state.players.len(),
                {
                    let cap: u64 = output.capacity().unpack();
                    cap / CKB
                },
            );
            let tip = GameTip { out_point, output, state };

            let cell_deps = vec![
                dep(game_dep, DepType::Code),      // game type script
                dep(auth_dep, DepType::Code),      // ckb-auth (spawned per intent)
                dep(secp_dep, DepType::DepGroup),  // the game cell's sighash lock
            ];
            (
                GameOperator::new(game_id, cell_deps, tip).with_fee(fee),
                Box::new(rpc),
                Box::new(move |tx| sign_sighash_all(tx, &sk).map_err(|e| e.to_string())),
            )
        }
        other => return Err(anyhow!("CHAIN={other} not supported (mock | http)")),
    };

    // Invoice relay queue (in-memory) + match-history log (JSONL). Neither touches
    // funds — the relay shuttles invoice STRINGS, the log records what settled.
    let mut relay = Relay::new();
    let results = ResultsLog {
        path: std::path::PathBuf::from(env_or("RESULTS_FILE", "game-results.jsonl")),
    };

    let server = Server::http(&listen).map_err(|e| anyhow!("bind {listen}: {e}"))?;
    println!("game-operator listening on http://{listen}");
    println!("  game_id: 0x{}", hex::encode(game_id));
    println!("  chain:   {chain}");
    println!("  results: {}", results.path.display());

    for mut request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("").to_string();

        // CORS preflight.
        if method == Method::Options {
            let _ = request.respond(json_response(204, serde_json::json!({})));
            continue;
        }

        let mut body = String::new();
        if request.as_reader().read_to_string(&mut body).is_err() {
            let _ = request.respond(json_response(400, serde_json::json!({ "error": "unreadable body" })));
            continue;
        }

        let (status, payload) = match (&method, path.as_str()) {
            (Method::Get, "/health") => (
                200,
                serde_json::json!({
                    "status": "ok",
                    "game_id": format!("0x{}", hex::encode(game_id)),
                    "seq": operator.tip().state.seq,
                    "pending": operator.pending(),
                    "invoices": relay.invoices.len(),
                }),
            ),
            (Method::Get, "/game") => (200, state_json(&operator.tip().state)),
            (Method::Post, "/intent") => {
                handle_intent(&mut operator, rpc.as_ref(), &finalize, &results, &body)
            }
            (Method::Post, "/flush") => handle_flush(&mut operator, rpc.as_ref(), &finalize),
            // --- invoice relay (Phase 3) — strings only, never funds ---
            (Method::Post, "/invoice") => handle_publish_invoice(&mut relay, &results, &body),
            (Method::Get, "/invoice") => {
                let payer = query_param(&url, "for");
                match relay.next_for(payer.as_deref()) {
                    Some(inv) => (200, serde_json::json!({ "invoice": inv.to_json() })),
                    None => (200, serde_json::json!({ "invoice": serde_json::Value::Null })),
                }
            }
            (Method::Post, "/invoice/paid") => {
                let seq = operator.tip().state.seq;
                handle_mark_paid(&mut relay, &results, seq, &body)
            }
            (Method::Get, "/results") => {
                let n = query_param(&url, "n").and_then(|s| s.parse().ok()).unwrap_or(50);
                (200, serde_json::json!({ "results": results.last(n) }))
            }
            _ => (404, serde_json::json!({ "error": "not found" })),
        };

        let _ = request.respond(json_response(status, payload));
    }
    Ok(())
}
