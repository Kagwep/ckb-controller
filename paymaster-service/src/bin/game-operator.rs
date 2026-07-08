//! Game aggregator operator — the runnable HTTP front door for [`GameOperator`].
//!
//! This is the sequencing service N players talk to: each posts a session-signed
//! intent, the operator batches it into a game-cell transition and advances the
//! shared board. It exposes the loop over HTTP (JSON, CORS-enabled so the browser
//! demo can call it directly):
//!
//!   GET  /health   -> { status, game_id, seq, pending }
//!   GET  /game     -> { seq, players: [ { hash, score, nonce } ] }
//!   POST /intent   -> { intent: "0x<101 bytes>" }  => { tx_hash, seq }   (submit + flush)
//!   POST /flush    -> {}                            => { tx_hash, seq }   (flush pending)
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

    // Admission-validate the intent (stale nonce / over-cap rejected here, before
    // it can jam the queue). Signatures are enforced on-chain, not here.
    if let Err(e) = op.submit(intent) {
        return (op_error_status(&e), serde_json::json!({ "error": format!("{e:?}") }));
    }
    // Auto-flush this move into its own transition (the demo shows one step per
    // move; batching several submits before a single /flush is also supported).
    match flush_or_shed(op, rpc, finalize) {
        Ok(hash) => (
            200,
            serde_json::json!({
                "tx_hash": format!("{:#x}", hash),
                "seq": op.tip().state.seq,
            }),
        ),
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

    let server = Server::http(&listen).map_err(|e| anyhow!("bind {listen}: {e}"))?;
    println!("game-operator listening on http://{listen}");
    println!("  game_id: 0x{}", hex::encode(game_id));
    println!("  chain:   {chain}");

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
                }),
            ),
            (Method::Get, "/game") => (200, state_json(&operator.tip().state)),
            (Method::Post, "/intent") => handle_intent(&mut operator, rpc.as_ref(), &finalize, &body),
            (Method::Post, "/flush") => handle_flush(&mut operator, rpc.as_ref(), &finalize),
            _ => (404, serde_json::json!({ "error": "not found" })),
        };

        let _ = request.respond(json_response(status, payload));
    }
    Ok(())
}
