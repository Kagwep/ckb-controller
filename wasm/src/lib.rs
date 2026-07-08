//! Browser/JS client for the controller session lock — a `wasm-bindgen` surface
//! over [`controller_sdk`].
//!
//! Everything crosses the JS boundary as hex strings (`0x…`) and plain integers,
//! so a TypeScript app can drive the full controller flow client-side without a
//! Rust toolchain:
//!
//! * **encoding** — [`session_params`], [`registered_args`], [`owner_only_args`],
//!   [`script`] (molecule-encode a CKB `Script`);
//! * **address** — [`controller_address`] (RFC21 full address, bech32m);
//! * **messages** — [`tx_message`] (the cell_deps-cleared signing message),
//!   [`session_auth_message`] (carried model);
//! * **policy** — [`policy_leaf`], [`merkle_root`], [`merkle_proof`],
//!   [`merkle_verify`];
//! * **witnesses** — [`owner_witness`], [`session_witness_registered`],
//!   [`session_witness_carried`], [`channel_proof_region`];
//! * **channels** — [`ChannelSession`], the open→pay→close loop on an in-memory
//!   rail, so a browser demo runs with no node. Production swaps the rail for
//!   Fiber's JS light client on the JS side.
//!
//! Errors cross the boundary as JS exceptions (thrown strings). The caller signs
//! the messages this crate computes (recoverable secp256k1, 65 bytes) and feeds
//! the signatures back into the witness builders — the SDK and this wrapper stay
//! agnostic to the JS key manager / wallet in use.

use ckb_types::{
    bytes::Bytes,
    packed::{Byte32, CellDep, OutPoint, Script, Transaction},
    prelude::*,
};
use controller_sdk as sdk;
use wasm_bindgen::prelude::*;

// `Result<_, String>` (not `JsError`) so error paths are unit-testable on the
// host too; wasm-bindgen still surfaces the string as a thrown JS error.
type R<T> = Result<T, String>;

// ---- hex helpers -----------------------------------------------------------

fn hx(s: &str) -> R<Vec<u8>> {
    hex::decode(s.trim_start_matches("0x")).map_err(|e| format!("invalid hex: {e}"))
}
fn arr20(s: &str) -> R<[u8; 20]> {
    hx(s)?.try_into().map_err(|_| "expected 20 bytes".to_string())
}
fn arr32(s: &str) -> R<[u8; 32]> {
    hx(s)?.try_into().map_err(|_| "expected 32 bytes".to_string())
}
fn hexout(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}
fn parse_u128(s: &str) -> R<u128> {
    s.parse::<u128>().map_err(|e| format!("invalid u128 '{s}': {e}"))
}

// ---- sentinels (functions; wasm-bindgen can't export u128 consts) -----------

/// Policies root meaning "any policy allowed".
#[wasm_bindgen]
pub fn wildcard_root() -> String {
    hexout(&sdk::WILDCARD_ROOT)
}
/// Spend cap meaning "no limit", as a decimal string (u128::MAX).
#[wasm_bindgen]
pub fn spend_cap_unlimited() -> String {
    sdk::SPEND_CAP_UNLIMITED.to_string()
}
/// Expiry meaning "never expires" (the lock skips its header-dep read).
#[wasm_bindgen]
pub fn no_expiry() -> u64 {
    sdk::NO_EXPIRY
}

// ---- encoding --------------------------------------------------------------

/// Molecule-encode a CKB `Script` from its parts; returns the serialized bytes
/// as hex (the form [`ChannelSession::new`] and CKB tx builders consume).
#[wasm_bindgen]
pub fn script(code_hash_hex: &str, hash_type: u8, args_hex: &str) -> R<String> {
    let s = Script::new_builder()
        .code_hash(arr32(code_hash_hex)?.pack())
        .hash_type(hash_type.into())
        .args(Bytes::from(hx(args_hex)?).pack())
        .build();
    Ok(hexout(s.as_slice()))
}

/// The 96-byte session-params block.
#[wasm_bindgen]
pub fn session_params(
    session_pubkey_hash: &str,
    expires_at: u64,
    policies_root: &str,
    spend_cap: &str,
    guardian_pubkey_hash: &str,
) -> R<String> {
    let p = sdk::session_params(
        &arr20(session_pubkey_hash)?,
        expires_at,
        &arr32(policies_root)?,
        parse_u128(spend_cap)?,
        &arr20(guardian_pubkey_hash)?,
    );
    Ok(hexout(&p))
}

/// Registered-model args: owner hash ‖ params (116 bytes).
#[wasm_bindgen]
pub fn registered_args(owner_pubkey_hash: &str, params: &str) -> R<String> {
    Ok(hexout(&sdk::registered_args(
        &arr20(owner_pubkey_hash)?,
        &hx(params)?,
    )))
}

/// Carried-model args: just the owner hash (20 bytes).
#[wasm_bindgen]
pub fn owner_only_args(owner_pubkey_hash: &str) -> R<String> {
    Ok(hexout(&sdk::owner_only_args(&arr20(owner_pubkey_hash)?)))
}

// ---- address ---------------------------------------------------------------

/// RFC21 full address: `bech32m(0x00 ‖ code_hash ‖ hash_type ‖ args)`. `testnet`
/// chooses the `ckt`/`ckb` hrp. `hash_type` is the raw byte (e.g. 0x04 = data2,
/// which the controller lock uses on CKB-VM v2).
#[wasm_bindgen]
pub fn controller_address(
    code_hash_hex: &str,
    hash_type: u8,
    args_hex: &str,
    testnet: bool,
) -> R<String> {
    use bech32::{ToBase32, Variant};
    let mut payload = vec![0x00u8];
    payload.extend_from_slice(&arr32(code_hash_hex)?);
    payload.push(hash_type);
    payload.extend_from_slice(&hx(args_hex)?);
    let hrp = if testnet { "ckt" } else { "ckb" };
    bech32::encode(hrp, payload.to_base32(), Variant::Bech32m).map_err(|e| format!("bech32: {e}"))
}

// ---- messages --------------------------------------------------------------

/// The session/owner signing message: blake2b of the raw tx with cell_deps
/// cleared. `tx_molecule_hex` is the molecule-serialized `Transaction` (what
/// `ckb-sdk-js` produces from a built tx).
#[wasm_bindgen]
pub fn tx_message(tx_molecule_hex: &str) -> R<String> {
    let tx = Transaction::from_slice(&hx(tx_molecule_hex)?)
        .map_err(|e| format!("invalid transaction: {e}"))?
        .into_view();
    Ok(hexout(&sdk::tx_message(&tx)))
}

/// The carried-model owner-authorization message.
#[wasm_bindgen]
pub fn session_auth_message(script_hash: &str, revocation_epoch: u64, params: &str) -> R<String> {
    Ok(hexout(&sdk::session_auth_message(
        &arr32(script_hash)?,
        revocation_epoch,
        &hx(params)?,
    )))
}

// ---- policy Merkle ---------------------------------------------------------

/// Policy leaf for a script hash (works for both TYPE and LOCK policies).
#[wasm_bindgen]
pub fn policy_leaf(script_hash: &str) -> R<String> {
    Ok(hexout(&sdk::policy_leaf(&arr32(script_hash)?)))
}

fn leaves_from(hexes: &[String]) -> R<Vec<[u8; 32]>> {
    hexes.iter().map(|h| arr32(h)).collect()
}

/// Merkle root over policy leaves (sorted-pair, zero-padded). Takes a JS array of
/// 32-byte hex leaves.
#[wasm_bindgen]
pub fn merkle_root(leaves: Vec<String>) -> R<String> {
    Ok(hexout(&sdk::merkle_root(&leaves_from(&leaves)?)))
}

/// Membership proof for `leaves[index]`: a JS array of sibling-hash hex strings.
#[wasm_bindgen]
pub fn merkle_proof(leaves: Vec<String>, index: usize) -> R<Vec<String>> {
    Ok(sdk::merkle_proof(&leaves_from(&leaves)?, index)
        .iter()
        .map(|s| hexout(s))
        .collect())
}

/// Verify a proof (mirror of the on-chain check), for client-side self-tests.
#[wasm_bindgen]
pub fn merkle_verify(root: &str, leaf: &str, proof: Vec<String>) -> R<bool> {
    let proof: Vec<[u8; 32]> = proof.iter().map(|s| arr32(s)).collect::<R<_>>()?;
    Ok(sdk::merkle_verify(&arr32(root)?, &arr32(leaf)?, &proof))
}

// ---- witnesses -------------------------------------------------------------

/// OWNER-mode witness (`WitnessArgs.lock`).
#[wasm_bindgen]
pub fn owner_witness(owner_sig: &str) -> R<String> {
    Ok(hexout(&sdk::owner_witness(&hx(owner_sig)?)))
}

/// SESSION-mode witness, registered model. `guardian_sig` may be empty.
#[wasm_bindgen]
pub fn session_witness_registered(
    session_sig: &str,
    guardian_sig: &str,
    proof_region: &str,
) -> R<String> {
    let g = hx(guardian_sig)?;
    let guardian = if g.is_empty() { None } else { Some(g.as_slice()) };
    Ok(hexout(&sdk::session_witness_registered(
        &hx(session_sig)?,
        guardian,
        &hx(proof_region)?,
    )))
}

/// SESSION-mode witness, authorization-carried model. `guardian_sig` may be empty.
#[wasm_bindgen]
pub fn session_witness_carried(
    params: &str,
    owner_auth: &str,
    session_sig: &str,
    guardian_sig: &str,
    proof_region: &str,
) -> R<String> {
    let g = hx(guardian_sig)?;
    let guardian = if g.is_empty() { None } else { Some(g.as_slice()) };
    Ok(hexout(&sdk::session_witness_carried(
        &hx(params)?,
        &hx(owner_auth)?,
        &hx(session_sig)?,
        guardian,
        &hx(proof_region)?,
    )))
}

/// The proof region for a channel funding tx (one empty LOCK-kind entry).
#[wasm_bindgen]
pub fn channel_proof_region() -> String {
    hexout(&sdk::channel::channel_proof_region())
}

/// Channel session params: scoped to a single funding lock, spend cap = budget.
#[wasm_bindgen]
pub fn channel_session_params(
    session_pubkey_hash: &str,
    expires_at: u64,
    funding_lock_script: &str,
    budget: u64,
    guardian_pubkey_hash: &str,
) -> R<String> {
    let funding_lock = Script::from_slice(&hx(funding_lock_script)?)
        .map_err(|e| format!("invalid funding lock script: {e}"))?;
    Ok(hexout(&sdk::channel::channel_session_params(
        &arr20(session_pubkey_hash)?,
        expires_at,
        &funding_lock,
        budget,
        &arr20(guardian_pubkey_hash)?,
    )))
}

// ---- game intents (aggregator multiplayer) ---------------------------------
//
// The browser half of the aggregator: a player builds ONE session-signed intent
// and posts it to the operator, which batches intents into a game-cell transition.
// `game_intent_message` is what the session key signs (recoverable secp256k1, 65
// bytes); feed the signature into `game_encode_intent` to get the 101-byte intent
// the operator's /intent endpoint expects. `game_decode_state` renders the shared
// board (a game cell's data) for display. Mirrors `controller_sdk::game`, which is
// drift-guarded against the on-chain type script in CKB-VM.

/// The message a player signs to authorize a move:
/// `blake2b_256(DOMAIN ‖ game_id ‖ player_hash ‖ points ‖ nonce)`.
#[wasm_bindgen]
pub fn game_intent_message(
    game_id: &str,
    player_pubkey_hash: &str,
    points: u64,
    nonce: u64,
) -> R<String> {
    Ok(hexout(&sdk::game::intent_message(
        &arr32(game_id)?,
        &arr20(player_pubkey_hash)?,
        points,
        nonce,
    )))
}

/// Serialize a signed intent to the 101-byte wire form the operator ingests:
/// `player_hash(20) ‖ points(8) ‖ nonce(8) ‖ sig(65)`.
#[wasm_bindgen]
pub fn game_encode_intent(
    player_pubkey_hash: &str,
    points: u64,
    nonce: u64,
    sig: &str,
) -> R<String> {
    let sig: [u8; sdk::game::SIGNATURE_LEN] = hx(sig)?
        .try_into()
        .map_err(|_| "expected 65-byte signature".to_string())?;
    let intent = sdk::game::Intent {
        hash: arr20(player_pubkey_hash)?,
        points,
        nonce,
        sig,
    };
    Ok(hexout(&intent.encode()))
}

/// Apply an intent batch to a state, returning the NEXT state's encoded data hex
/// — the transition the operator writes to the game cell's output. `state` is the
/// current game cell data hex (empty = fresh); `batch` is the framed intent batch
/// (`n(2 LE) ‖ intents`). Uses the same rule as the on-chain type script (which is
/// drift-guarded against `controller_sdk::game`), so the result matches byte-for-
/// byte what the script recomputes — a live operator/driver computes output data
/// with this.
#[wasm_bindgen]
pub fn game_apply(state: &str, batch: &str) -> R<String> {
    let mut s = sdk::game::GameState::decode(&hx(state)?).map_err(|e| format!("bad state: {e:?}"))?;
    let intents = sdk::game::decode_batch(&hx(batch)?).map_err(|e| format!("bad batch: {e:?}"))?;
    s.apply_batch(&intents).map_err(|e| format!("apply failed: {e:?}"))?;
    Ok(hexout(&s.encode()))
}

/// Frame intents (each the 101-byte hex from [`game_encode_intent`]) into the batch
/// wire form `n(2 LE) ‖ intents` the operator/type-script expect.
#[wasm_bindgen]
pub fn game_encode_batch(intents: Vec<String>) -> R<String> {
    let mut parsed = Vec::with_capacity(intents.len());
    for i in &intents {
        parsed.push(sdk::game::Intent::decode(&hx(i)?).map_err(|e| format!("bad intent: {e:?}"))?);
    }
    Ok(hexout(&sdk::game::encode_batch(&parsed)))
}

/// Decode a game cell's `data` into a JSON board for display:
/// `{"seq":N,"players":[{"hash":"0x..","score":N,"nonce":N}, ...]}`.
#[wasm_bindgen]
pub fn game_decode_state(data: &str) -> R<String> {
    let state = sdk::game::GameState::decode(&hx(data)?).map_err(|e| format!("bad state: {e:?}"))?;
    let players: Vec<String> = state
        .players
        .iter()
        .map(|p| {
            format!(
                r#"{{"hash":"{}","score":{},"nonce":{}}}"#,
                hexout(&p.hash),
                p.score,
                p.nonce
            )
        })
        .collect();
    Ok(format!(
        r#"{{"seq":{},"players":[{}]}}"#,
        state.seq,
        players.join(",")
    ))
}

// ---- channel session (open -> pay -> close) --------------------------------

/// A built channel transaction crossing into JS: the molecule-serialized partial
/// `tx`, the `message` to session-sign, and the `outpoint` of the cell of
/// interest (funding cell on open, account cell on settle).
#[wasm_bindgen(getter_with_clone)]
#[derive(Clone)]
pub struct ChannelTx {
    pub tx: String,
    pub message: String,
    pub outpoint: String,
}

/// The result of [`ChannelSession::close`]: the net settlement (shannons, as
/// decimal strings) plus the settle [`ChannelTx`].
#[wasm_bindgen(getter_with_clone)]
pub struct CloseResult {
    pub local: String,
    pub remote: String,
    pub settle: ChannelTx,
}

fn channel_tx(ct: &sdk::channel::ChannelTx) -> ChannelTx {
    ChannelTx {
        tx: hexout(ct.tx.data().as_slice()),
        message: hexout(&sdk::tx_message(&ct.tx)),
        outpoint: hexout(ct.outpoint().as_slice()),
    }
}

/// The controller's session-funded payment channel, in the browser: **open → pay
/// ×N → close**, on an in-memory rail (no node). The L1 funding/settle txs it
/// builds are real partials, ready for the JS app to session-sign and broadcast;
/// the off-chain `pay`s are tracked locally (in production a Fiber JS rail carries
/// them). Budget = the session's entire economic exposure.
#[wasm_bindgen]
pub struct ChannelSession {
    inner: Option<sdk::channel::ChannelSession<sdk::channel::MockRail>>,
}

#[wasm_bindgen]
impl ChannelSession {
    /// Build a session against the live account cell. `*_script` args are
    /// molecule-encoded `Script`s (see [`script`]); `account_input` is
    /// `0x<txhash>:<index>`; `header_dep` is a 32-byte block hash hex (use the
    /// node's tip; pass all-zero for a `NO_EXPIRY` session that needs none).
    #[wasm_bindgen(constructor)]
    pub fn new(
        account_lock_script: &str,
        account_input: &str,
        account_capacity: u64,
        funding_lock_script: &str,
        header_dep: &str,
    ) -> R<ChannelSession> {
        let account_lock = Script::from_slice(&hx(account_lock_script)?)
            .map_err(|e| format!("invalid account lock: {e}"))?;
        let funding_lock = Script::from_slice(&hx(funding_lock_script)?)
            .map_err(|e| format!("invalid funding lock: {e}"))?;
        let cfg = sdk::channel::ChannelConfig {
            account_lock,
            account_input: parse_outpoint(account_input)?,
            account_capacity,
            funding_lock,
            cell_deps: Vec::<CellDep>::new(),
            header_dep: Byte32::from_slice(&arr32(header_dep)?).unwrap(),
        };
        Ok(ChannelSession {
            inner: Some(sdk::channel::ChannelSession::new(
                cfg,
                sdk::channel::MockRail::new(),
            )),
        })
    }

    /// **Open (1 L1 tx).** Returns the partial funding tx to session-sign + broadcast.
    pub fn open(&mut self, peer: &str, budget: u64) -> R<ChannelTx> {
        let s = self.inner.as_mut().ok_or_else(closed)?;
        let ct = s.open(&peer.into(), budget).map_err(|e| format!("open failed: {e:?}"))?;
        Ok(channel_tx(&ct))
    }

    /// **Pay (0 L1).** An off-chain micropayment; rejected if over budget.
    pub fn pay(&mut self, amount: u64) -> R<()> {
        let s = self.inner.as_mut().ok_or_else(closed)?;
        s.pay(amount as u128).map_err(|e| format!("pay failed: {e:?}"))
    }

    /// **Close (1 L1 tx).** Tears down the channel; returns the settlement + the
    /// partial settle tx. Consumes the session (further calls error).
    pub fn close(&mut self) -> R<CloseResult> {
        let s = self.inner.take().ok_or_else(closed)?;
        let (settlement, ct) = s.close().map_err(|e| format!("close failed: {e:?}"))?;
        Ok(CloseResult {
            local: settlement.local.to_string(),
            remote: settlement.remote.to_string(),
            settle: channel_tx(&ct),
        })
    }

    /// Remaining budget, shannons (decimal string).
    pub fn remaining(&self) -> String {
        self.inner.as_ref().map(|s| s.remaining()).unwrap_or(0).to_string()
    }
    /// Spent so far, shannons (decimal string).
    pub fn spent(&self) -> String {
        self.inner.as_ref().map(|s| s.spent()).unwrap_or(0).to_string()
    }
    pub fn is_open(&self) -> bool {
        self.inner.as_ref().map(|s| s.is_open()).unwrap_or(false)
    }
}

fn closed() -> String {
    "channel session is closed".to_string()
}

fn parse_outpoint(s: &str) -> R<OutPoint> {
    let (tx, idx) = s
        .split_once(':')
        .ok_or_else(|| "outpoint must be 0x<txhash>:<index>".to_string())?;
    let index: u32 = idx.parse().map_err(|e| format!("bad outpoint index: {e}"))?;
    Ok(OutPoint::new(arr32(tx)?.pack(), index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::core::TransactionBuilder;

    const Z32: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn args_roundtrip_matches_sdk() {
        let params = session_params(
            &hexout(&[1u8; 20]),
            42,
            &wildcard_root(),
            "7",
            &hexout(&[0u8; 20]),
        )
        .unwrap();
        let args = registered_args(&hexout(&[9u8; 20]), &params).unwrap();
        assert_eq!(args.len(), 2 + sdk::REGISTERED_ARGS_LEN * 2);
        let direct = sdk::registered_args(
            &[9u8; 20],
            &sdk::session_params(&[1u8; 20], 42, &sdk::WILDCARD_ROOT, 7, &[0u8; 20]),
        );
        assert_eq!(args, hexout(&direct));
    }

    #[test]
    fn bad_hex_is_rejected() {
        assert!(session_params("nothex", 1, &wildcard_root(), "1", &hexout(&[0u8; 20])).is_err());
        assert!(registered_args(&hexout(&[1u8; 5]), "0x00").is_err()); // owner hash not 20 bytes
    }

    #[test]
    fn address_is_bech32m() {
        let testnet = controller_address(Z32, 0x04, "0x1234", true).unwrap();
        assert!(testnet.starts_with("ckt1"));
        let mainnet = controller_address(Z32, 0x04, "0x1234", false).unwrap();
        assert!(mainnet.starts_with("ckb1"));
    }

    #[test]
    fn tx_message_matches_sdk() {
        let tx = TransactionBuilder::default()
            .output(
                ckb_types::packed::CellOutput::new_builder()
                    .capacity(1000u64.pack())
                    .build(),
            )
            .output_data(Bytes::new().pack())
            .build();
        let mol = hexout(tx.data().as_slice());
        assert_eq!(tx_message(&mol).unwrap(), hexout(&sdk::tx_message(&tx)));
    }

    #[test]
    fn merkle_root_proof_verify() {
        let leaves: Vec<String> = (0u8..4).map(|i| hexout(&[i; 32])).collect();
        let root = merkle_root(leaves.clone()).unwrap();
        let proof = merkle_proof(leaves.clone(), 2).unwrap();
        assert!(merkle_verify(&root, &leaves[2], proof).unwrap());
    }

    #[test]
    fn full_channel_loop_in_wasm_surface() {
        let funding = script(Z32, 0x00, "0x6669626572").unwrap(); // "fiber"
        let account = script(Z32, 0x04, "0x6163636f756e74").unwrap(); // "account"
        let mut s = ChannelSession::new(
            &account,
            &format!("{Z32}:0"),
            100_000_000_000, // 1000 CKB
            &funding,
            Z32,
        )
        .unwrap();

        let open = s.open("game-node", 50_000_000_000).unwrap();
        assert!(open.tx.starts_with("0x"));
        assert_eq!(open.message.len(), 66); // 0x + 32 bytes
        assert!(s.is_open());

        for _ in 0..5 {
            s.pay(1_000_000_000).unwrap();
        }
        assert_eq!(s.spent(), "5000000000");
        assert_eq!(s.remaining(), "45000000000");

        let close = s.close().unwrap();
        assert_eq!(close.local, "45000000000");
        assert_eq!(close.remote, "5000000000");
        assert!(close.settle.tx.starts_with("0x"));
        assert!(!s.is_open()); // consumed
    }

    #[test]
    fn game_intent_message_matches_sdk() {
        let game_id = hexout(&[7u8; 32]);
        let player = hexout(&[3u8; 20]);
        let msg = game_intent_message(&game_id, &player, 42, 5).unwrap();
        let direct = sdk::game::intent_message(&[7u8; 32], &[3u8; 20], 42, 5);
        assert_eq!(msg, hexout(&direct));
    }

    #[test]
    fn game_encode_intent_is_101_bytes() {
        let intent = game_encode_intent(&hexout(&[3u8; 20]), 10, 1, &hexout(&[0u8; 65])).unwrap();
        // 0x + 101 bytes * 2 hex chars
        assert_eq!(intent.len(), 2 + sdk::game::INTENT_LEN * 2);
        assert!(game_encode_intent(&hexout(&[3u8; 20]), 10, 1, &hexout(&[0u8; 10])).is_err());
    }

    #[test]
    fn game_apply_and_batch_match_sdk() {
        // build two intents via the surface, frame them, apply to empty state.
        let i1 = game_encode_intent(&hexout(&[1u8; 20]), 10, 1, &hexout(&[0u8; 65])).unwrap();
        let i2 = game_encode_intent(&hexout(&[2u8; 20]), 20, 1, &hexout(&[0u8; 65])).unwrap();
        let batch = game_encode_batch(vec![i1, i2]).unwrap();
        let next = game_apply("0x", &batch).unwrap();

        // compare against the SDK directly.
        let mut s = sdk::game::GameState::empty();
        s.apply_batch(&[
            sdk::game::Intent { hash: [1u8; 20], points: 10, nonce: 1, sig: [0u8; 65] },
            sdk::game::Intent { hash: [2u8; 20], points: 20, nonce: 1, sig: [0u8; 65] },
        ])
        .unwrap();
        assert_eq!(next, hexout(&s.encode()));

        // and the rendered board reflects it.
        let json = game_decode_state(&next).unwrap();
        assert!(json.contains(r#""seq":2"#));
    }

    #[test]
    fn game_decode_state_renders_board() {
        // build a state via the SDK, decode through the wasm surface.
        let mut s = sdk::game::GameState::empty();
        s.apply_batch(&[sdk::game::Intent { hash: [9u8; 20], points: 30, nonce: 1, sig: [0u8; 65] }])
            .unwrap();
        let json = game_decode_state(&hexout(&s.encode())).unwrap();
        assert!(json.contains(r#""seq":1"#));
        assert!(json.contains(r#""score":30"#));
        assert!(json.contains(&hexout(&[9u8; 20])));
    }

    #[test]
    fn pay_over_budget_errors() {
        let funding = script(Z32, 0x00, "0x00").unwrap();
        let account = script(Z32, 0x04, "0x00").unwrap();
        let mut s =
            ChannelSession::new(&account, &format!("{Z32}:0"), 1000, &funding, Z32).unwrap();
        s.open("peer", 100).unwrap();
        assert!(s.pay(1000).is_err()); // exceeds budget
    }
}
