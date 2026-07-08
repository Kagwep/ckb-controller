#![no_std]
#![cfg_attr(not(test), no_main)]

//! Cartridge-Controller-style **session lock** for CKB.
//!
//! One pop-up authorizes a temporary session key; the game then signs
//! transactions silently within tight, on-chain-enforced limits. This is the
//! CKB analog of the Starknet controller session token.
//!
//! Modeled on `nervosnetwork/fiber-scripts` (funding-lock / commitment-lock):
//! - signature verification is delegated to the **ckb-auth** binary via
//!   `spawn_cell` (algorithm id 0 = CKB secp256k1, 7 = Schnorr),
//! - the signing message is `blake2b_256(raw_tx with cell_deps cleared)` so a
//!   relayer/paymaster may attach fee cell-deps without breaking the signature,
//! - account continuity ("a session may not administer the account") is enforced
//!   by checking the output re-creates this same lock with unchanged owner args.
//!
//! This is a **skeleton**: the control flow and parsing are wired; the spots that
//! need real crypto/proof logic are marked `TODO`.

#[cfg(test)]
extern crate alloc;

use ckb_hash::blake2b_256;
#[cfg(not(test))]
use ckb_std::default_alloc;
#[cfg(not(test))]
ckb_std::entry!(program_entry);
#[cfg(not(test))]
default_alloc!();

use alloc::ffi::CString;
use ckb_std::{
    ckb_constants::Source,
    ckb_types::{bytes::Bytes, core::ScriptHashType, prelude::*},
    error::SysError,
    high_level::{
        load_cell_capacity, load_cell_data, load_cell_lock, load_cell_type, load_header,
        load_input_since, load_script, load_script_hash, load_transaction, load_witness_args,
        spawn_cell, QueryIter,
    },
    syscalls::wait,
};
use hex::encode;

include!(concat!(env!("OUT_DIR"), "/auth_code_hash.rs"));

// ---- Args layout (read via load_script) -----------------------------------
// The account cell's args pick the trust model by their length:
//
// REGISTERED model — args = owner_pubkey_hash(20) ‖ <session params 96> = 116 bytes.
//   The owner baked the session into the cell (one on-chain tx); the lock trusts
//   the params because only OWNER mode can rewrite args.
//
// AUTHORIZATION-CARRIED model — args = owner_pubkey_hash(20) = 20 bytes.
//   No on-chain session setup. The session params + an owner signature blessing
//   them ride in the witness and are re-verified every tx (cf. Starknet
//   controller's cached authorization).
//
// "Session params" (96 bytes) are the same in both, and equal args[20..116] in
// the registered model:
//   session_pubkey_hash(20) ‖ expires_at(8 LE) ‖ allowed_policies_root(32)
//     ‖ spend_cap(16 LE u128) ‖ guardian_pubkey_hash(20)
//   guardian all-zero = no guardian; otherwise a guardian co-signature is required.
const ARGS_LEN: usize = 116; // registered model
const OWNER_ONLY_ARGS_LEN: usize = 20; // authorization-carried model
const OWNER_HASH: core::ops::Range<usize> = 0..20;

// Offsets WITHIN a 96-byte session-params block.
const SESSION_PARAMS_LEN: usize = 96;
const SP_SESSION_HASH: core::ops::Range<usize> = 0..20;
const SP_EXPIRES_AT: core::ops::Range<usize> = 20..28;
const SP_POLICIES_ROOT: core::ops::Range<usize> = 28..60;
const SP_SPEND_CAP: core::ops::Range<usize> = 60..76;
const SP_GUARDIAN_HASH: core::ops::Range<usize> = 76..96;

// spend_cap sentinel meaning "no limit".
const SPEND_CAP_UNLIMITED: u128 = u128::MAX;
/// Sentinel expiry meaning "never expires": the lock skips the header-dep read,
/// so the session can authorize a counterparty-built tx that carries no header
/// dep (e.g. a Fiber funding tx).
const NO_EXPIRY: u64 = u64::MAX;

// Domain separator for the owner's session authorization (carried model), so the
// signature can never be confused with a tx signature or reused cross-protocol.
const SESSION_AUTH_DOMAIN: &[u8] = b"ckb-controller/session-auth/v1";
// The carried owner-authorization is bound to a monotonic revocation epoch read
// from the account cell's DATA (first 8 bytes, LE; empty = 0). Bumping the epoch
// via an OWNER tx invalidates all carried authorizations signed for older epochs
// — the carried-model "revoke before expiry" mechanism. (Data, unlike args, does
// not change the lock script hash / account address.)
const REVOCATION_EPOCH_LEN: usize = 8;
const AUTH_MSG_BUF_LEN: usize =
    SESSION_AUTH_DOMAIN.len() + 32 + REVOCATION_EPOCH_LEN + SESSION_PARAMS_LEN;

// Policy dimensions for a non-wildcard session: each constrained output is
// allowed by its TYPE-script hash or its LOCK-script hash. Lock policies let a
// session be scoped to e.g. "may only fund the Fiber channel funding-lock".
const POLICY_KIND_TYPE: u8 = 0;
const POLICY_KIND_LOCK: u8 = 1;

// Sentinel root meaning "any policy allowed" (frictionless onboarding).
// Prefer a real root in production.
const WILDCARD_ROOT: [u8; 32] = [0xFFu8; 32];

const SIGNATURE_LEN: usize = 65;
const MODE_OWNER: u8 = 0;
const MODE_SESSION: u8 = 1;

// ckb-auth algorithm ids
const AUTH_ID_CKB_SECP256K1: u8 = 0;

#[repr(i8)]
pub enum Error {
    IndexOutOfBound = 1,
    ItemMissing,
    LengthNotEnough,
    Encoding,
    // -- customized --
    MultipleInputs,
    ArgsLenError,
    EmptyWitnessError,
    WitnessLenError,
    InvalidMode,
    WrongSessionKey,
    SessionDisabled,
    SessionExpired,
    PolicyNotAllowed,
    PolicyProofMissing,
    PolicyProofMalformed,
    SessionCannotAdminister,
    SpendCapExceeded,
    AuthError,
}

impl From<SysError> for Error {
    fn from(err: SysError) -> Self {
        match err {
            SysError::IndexOutOfBound => Self::IndexOutOfBound,
            SysError::ItemMissing => Self::ItemMissing,
            SysError::LengthNotEnough(_) => Self::LengthNotEnough,
            SysError::Encoding => Self::Encoding,
            SysError::Unknown(code) => panic!("unexpected sys error {}", code),
            _ => panic!("unreachable spawn-related sys error"),
        }
    }
}

pub fn program_entry() -> i8 {
    match auth() {
        Ok(_) => 0,
        Err(err) => err as i8,
    }
}

fn auth() -> Result<(), Error> {
    // Account cells using this lock are expected to be spent one-per-group.
    if load_input_since(1, Source::GroupInput).is_ok() {
        return Err(Error::MultipleInputs);
    }

    let script = load_script()?;
    let args: Bytes = script.args().unpack();
    if args.len() != ARGS_LEN && args.len() != OWNER_ONLY_ARGS_LEN {
        return Err(Error::ArgsLenError);
    }

    // Unlock data lives in WitnessArgs.lock of the first group input.
    let witness_args = load_witness_args(0, Source::GroupInput)?;
    let lock_bytes: Bytes = witness_args
        .lock()
        .to_opt()
        .ok_or(Error::EmptyWitnessError)?
        .unpack();
    if lock_bytes.is_empty() {
        return Err(Error::EmptyWitnessError);
    }

    // Message commits to the whole tx EXCEPT cell_deps, so a paymaster can attach
    // fee-related cell_deps without invalidating the session signature.
    let message = tx_message();

    match lock_bytes[0] {
        MODE_OWNER => verify_owner(&args, &lock_bytes[1..], &message),
        MODE_SESSION => {
            let script_hash = load_script_hash()?;
            verify_session(&script, &script_hash, &args, &lock_bytes[1..], &message)
        }
        _ => Err(Error::InvalidMode),
    }
}

/// OWNER mode: full control. One interactive signature produced `sig`.
fn verify_owner(args: &[u8], rest: &[u8], message: &[u8; 32]) -> Result<(), Error> {
    if rest.len() < SIGNATURE_LEN {
        return Err(Error::WitnessLenError);
    }
    let sig = &rest[0..SIGNATURE_LEN];
    verify_signature(AUTH_ID_CKB_SECP256K1, sig, message, &args[OWNER_HASH])
}

/// Session parameters (caveats the owner authorized), resolved from either the
/// account args (registered model) or the witness (authorization-carried model).
struct SessionParams {
    session_pubkey_hash: [u8; 20],
    expires_at: u64,
    policies_root: [u8; 32],
    spend_cap: u128,
    guardian_pubkey_hash: [u8; 20],
}

fn parse_session_params(b: &[u8]) -> SessionParams {
    SessionParams {
        session_pubkey_hash: b[SP_SESSION_HASH].try_into().unwrap(),
        expires_at: u64::from_le_bytes(b[SP_EXPIRES_AT].try_into().unwrap()),
        policies_root: b[SP_POLICIES_ROOT].try_into().unwrap(),
        spend_cap: u128::from_le_bytes(b[SP_SPEND_CAP].try_into().unwrap()),
        guardian_pubkey_hash: b[SP_GUARDIAN_HASH].try_into().unwrap(),
    }
}

/// The message the OWNER signs to bless a session (carried model). Bound to this
/// exact account lock (via its script hash), the current revocation epoch, and
/// the session params, with a domain separator. NOT tx-specific: one owner
/// signature authorizes the session across many transactions; per-tx freshness
/// comes from `session_signature`. Binding the epoch is what makes carried
/// sessions revocable — see `current_revocation_epoch`.
fn session_auth_message(
    script_hash: &[u8; 32],
    revocation_epoch: u64,
    params_block: &[u8],
) -> [u8; 32] {
    let mut buf = [0u8; AUTH_MSG_BUF_LEN];
    let d = SESSION_AUTH_DOMAIN.len();
    buf[0..d].copy_from_slice(SESSION_AUTH_DOMAIN);
    buf[d..d + 32].copy_from_slice(script_hash);
    buf[d + 32..d + 40].copy_from_slice(&revocation_epoch.to_le_bytes());
    buf[d + 40..d + 40 + SESSION_PARAMS_LEN].copy_from_slice(params_block);
    blake2b_256(buf)
}

/// The account's current revocation epoch = first 8 bytes (LE) of the GroupInput
/// account cell's data, or 0 if absent. Read from the real on-chain cell being
/// spent, so a session holder cannot forge it.
fn current_revocation_epoch() -> Result<u64, Error> {
    let data = load_cell_data(0, Source::GroupInput)?;
    Ok(if data.len() >= REVOCATION_EPOCH_LEN {
        u64::from_le_bytes(data[0..REVOCATION_EPOCH_LEN].try_into().unwrap())
    } else {
        0
    })
}

/// SESSION mode: silent gameplay path with on-chain guard rails.
fn verify_session(
    script: &ckb_std::ckb_types::packed::Script,
    script_hash: &[u8; 32],
    args: &[u8],
    rest: &[u8],
    message: &[u8; 32],
) -> Result<(), Error> {
    // Resolve the session params + locate the session signature and proof region,
    // per the trust model selected by the account's args length.
    // Resolve params + the signature region, per the trust model selected by the
    // account's args length. The signature region is
    //   session_signature(65) ‖ [guardian_signature(65)] ‖ proof_region
    let (params, sig_region) = match args.len() {
        // REGISTERED: params live in args[20..116]; witness = sig_region.
        ARGS_LEN => (parse_session_params(&args[OWNER_ONLY_ARGS_LEN..ARGS_LEN]), rest),
        // CARRIED: witness = params(96) ‖ owner_auth(65) ‖ sig_region.
        OWNER_ONLY_ARGS_LEN => {
            if rest.len() < SESSION_PARAMS_LEN + SIGNATURE_LEN {
                return Err(Error::WitnessLenError);
            }
            let params_block = &rest[0..SESSION_PARAMS_LEN];
            let owner_auth = &rest[SESSION_PARAMS_LEN..SESSION_PARAMS_LEN + SIGNATURE_LEN];

            // The owner must have blessed exactly these params for this account AT
            // THE CURRENT revocation epoch. If the owner has since bumped the epoch
            // (revoked), an authorization signed for an older epoch fails here.
            let epoch = current_revocation_epoch()?;
            let auth_msg = session_auth_message(script_hash, epoch, params_block);
            verify_signature(AUTH_ID_CKB_SECP256K1, owner_auth, &auth_msg, &args[OWNER_HASH])?;

            (
                parse_session_params(params_block),
                &rest[SESSION_PARAMS_LEN + SIGNATURE_LEN..],
            )
        }
        _ => return Err(Error::ArgsLenError),
    };

    // (0) a session must actually be enabled.
    if params.session_pubkey_hash == [0u8; 20] {
        return Err(Error::SessionDisabled);
    }

    if sig_region.len() < SIGNATURE_LEN {
        return Err(Error::WitnessLenError);
    }
    let session_sig = &sig_region[0..SIGNATURE_LEN];
    let mut cursor = SIGNATURE_LEN;

    // (1) the session key signs this tx.
    verify_signature(
        AUTH_ID_CKB_SECP256K1,
        session_sig,
        message,
        &params.session_pubkey_hash,
    )?;

    // (1b) optional guardian co-signature, if this session configured a guardian.
    if params.guardian_pubkey_hash != [0u8; 20] {
        if sig_region.len() < cursor + SIGNATURE_LEN {
            return Err(Error::WitnessLenError);
        }
        let guardian_sig = &sig_region[cursor..cursor + SIGNATURE_LEN];
        verify_signature(
            AUTH_ID_CKB_SECP256K1,
            guardian_sig,
            message,
            &params.guardian_pubkey_hash,
        )?;
        cursor += SIGNATURE_LEN;
    }

    let proof_region = &sig_region[cursor..];

    // (2) not expired. Upper-bound "now" via a header dep timestamp. The sentinel
    //     expires_at == NO_EXPIRY means "never expires": skip the header-dep read
    //     entirely, so the session can authorize a transaction a counterparty
    //     built without a header dep (e.g. a Fiber funding tx). A no-expiry
    //     session is still bounded by its spend cap and policy allowlist.
    if params.expires_at != NO_EXPIRY {
        let now_ms = current_timestamp_ms()?;
        if now_ms / 1000 >= params.expires_at {
            return Err(Error::SessionExpired);
        }
    }

    // (3) policy: the session may only shape cells it is allowed to.
    if params.policies_root != WILDCARD_ROOT {
        check_output_policies(script, &params.policies_root, proof_region)?;
    }

    // (4) a session may NOT administer the account: any output re-creating this
    //     lock must keep ALL account args intact. The same walk sums how much
    //     capacity stays under this lock, which check (5) needs.
    let returned_capacity = enforce_account_outputs(script, args)?;

    // (5) bound net value outflow from account cells (defense in depth vs a
    //     compromised session + malicious relayer).
    check_spend_cap(params.spend_cap, returned_capacity)?;

    Ok(())
}

/// blake2b_256 over the raw transaction with cell_deps cleared.
fn tx_message() -> [u8; 32] {
    let tx = load_transaction()
        .expect("load tx")
        .raw()
        .as_builder()
        .cell_deps(Default::default())
        .build();
    blake2b_256(tx.as_slice())
}

/// Delegate signature verification to the ckb-auth binary via spawn_cell, so we
/// can verify and then keep validating (vs exec_cell which replaces the process).
fn verify_signature(
    algorithm_id: u8,
    signature: &[u8],
    message: &[u8; 32],
    pubkey_hash: &[u8],
) -> Result<(), Error> {
    let algorithm_id_str = CString::new(encode([algorithm_id])).unwrap();
    let signature_str = CString::new(encode(signature)).unwrap();
    let message_str = CString::new(encode(message)).unwrap();
    let pubkey_hash_str = CString::new(encode(pubkey_hash)).unwrap();

    let auth_args = [
        algorithm_id_str.as_c_str(),
        signature_str.as_c_str(),
        message_str.as_c_str(),
        pubkey_hash_str.as_c_str(),
    ];

    let pid = spawn_cell(&AUTH_CODE_HASH, ScriptHashType::Data1, &auth_args, &[])
        .map_err(|_| Error::AuthError)?;
    let result = wait(pid).map_err(|_| Error::AuthError)?;
    if result != 0 {
        return Err(Error::AuthError);
    }
    Ok(())
}

/// When the session is scoped (root != WILDCARD), every output that is NOT the
/// account's own continuation must be explicitly allowed by the policy root —
/// either by its TYPE-script hash (kind 0) or its LOCK-script hash (kind 1). The
/// witness supplies, per such output in output order, a framed proof:
///   kind(1) ‖ proof_len(1) ‖ siblings(32·proof_len)
/// (the account's own outputs are skipped here; `enforce_account_outputs` governs
/// them via args/data equality).
///
/// The LOCK dimension is what lets a session be scoped to e.g. "may only move
/// value into the Fiber channel funding-lock" — it can fund a channel (or settle
/// back to the account) and nowhere else, even if the session key is compromised.
/// This also closes the prior hole where type-less outputs to arbitrary locks
/// were unconstrained (bounded only by the spend cap).
fn check_output_policies(
    script: &ckb_std::ckb_types::packed::Script,
    root: &[u8; 32],
    proof_region: &[u8],
) -> Result<(), Error> {
    let self_code = script.code_hash();
    let self_ht = script.hash_type();
    let mut cursor = 0usize;
    let mut index = 0usize;
    loop {
        let lock = match load_cell_lock(index, Source::Output) {
            Ok(l) => l,
            Err(SysError::IndexOutOfBound) => break,
            Err(e) => return Err(e.into()),
        };
        // Account self-output: governed by enforce_account_outputs, not policy.
        if lock.code_hash().as_slice() == self_code.as_slice()
            && lock.hash_type().as_slice() == self_ht.as_slice()
        {
            index += 1;
            continue;
        }

        // Non-self output: consume its framed policy proof.
        if cursor + 2 > proof_region.len() {
            return Err(Error::PolicyProofMissing);
        }
        let kind = proof_region[cursor];
        let proof_len = proof_region[cursor + 1] as usize;
        cursor += 2;
        let bytes = proof_len * 32;
        if cursor + bytes > proof_region.len() {
            return Err(Error::PolicyProofMalformed);
        }
        let proof = &proof_region[cursor..cursor + bytes];
        cursor += bytes;

        let leaf = match kind {
            POLICY_KIND_TYPE => match load_cell_type(index, Source::Output)? {
                Some(type_script) => blake2b_256(type_script.calc_script_hash().as_slice()),
                None => return Err(Error::PolicyNotAllowed),
            },
            POLICY_KIND_LOCK => blake2b_256(lock.calc_script_hash().as_slice()),
            _ => return Err(Error::PolicyProofMalformed),
        };
        if !merkle_verify(root, &leaf, proof) {
            return Err(Error::PolicyNotAllowed);
        }
        index += 1;
    }
    Ok(())
}

/// Walk outputs once: enforce that every output re-creating THIS lock keeps all
/// account args AND data byte-for-byte unchanged (a session may not administer
/// the account — only OWNER mode rewrites them), and return the total capacity
/// that stays under this lock (the account's continuing value).
///
/// Preserving data is what stops a session from resetting the revocation epoch to
/// evade a revoke; only OWNER mode (which skips this check) may change it.
fn enforce_account_outputs(
    script: &ckb_std::ckb_types::packed::Script,
    args: &[u8],
) -> Result<u128, Error> {
    // The single GroupInput account cell's data (revocation epoch lives here).
    let input_data = load_cell_data(0, Source::GroupInput)?;

    let mut returned_capacity: u128 = 0;
    let mut index = 0usize;
    loop {
        let lock = match load_cell_lock(index, Source::Output) {
            Ok(lock) => lock,
            Err(SysError::IndexOutOfBound) => break,
            Err(e) => return Err(e.into()),
        };
        let same_code = lock.code_hash().as_slice() == script.code_hash().as_slice()
            && lock.hash_type().as_slice() == script.hash_type().as_slice();
        if same_code {
            let out_args: Bytes = lock.args().unpack();
            // A session may not change ANY account arg. Full-equality is the safe
            // default; relaxing to "narrow-only" (e.g. shorter expiry, lower cap)
            // is a future refinement.
            if out_args.as_ref() != args {
                return Err(Error::SessionCannotAdminister);
            }
            // Nor may it change the account data (the revocation epoch).
            if load_cell_data(index, Source::Output)? != input_data {
                return Err(Error::SessionCannotAdminister);
            }
            let cap = load_cell_capacity(index, Source::Output)?;
            returned_capacity = returned_capacity.saturating_add(cap as u128);
        }
        index += 1;
    }
    Ok(returned_capacity)
}

/// Defense in depth: cap net CKB capacity outflow from account cells per tx.
///
/// `returned_capacity` is the capacity of outputs that stay under this lock
/// (from `enforce_account_outputs`). Net outflow = sum(account inputs) −
/// returned. UDT *amount* conservation is enforced by the UDT type script, not
/// here; this guards native CKB value leakage.
fn check_spend_cap(cap: u128, returned_capacity: u128) -> Result<(), Error> {
    if cap == SPEND_CAP_UNLIMITED {
        return Ok(());
    }

    let mut input_capacity: u128 = 0;
    for c in QueryIter::new(load_cell_capacity, Source::GroupInput) {
        input_capacity = input_capacity.saturating_add(c as u128);
    }

    let outflow = input_capacity.saturating_sub(returned_capacity);
    if outflow > cap {
        return Err(Error::SpendCapExceeded);
    }
    Ok(())
}

/// Upper-bound on "now": read a header dep's timestamp (ms since epoch).
fn current_timestamp_ms() -> Result<u64, Error> {
    let header = load_header(0, Source::HeaderDep)?;
    Ok(header.raw().timestamp().unpack())
}

/// Merkle membership check using **sorted-pair** hashing, so proofs carry no
/// direction bits: at each step the node and its sibling are ordered before
/// hashing. `proof` is a flat sequence of 32-byte sibling hashes (leaf → root).
///
/// Mirrors the Starknet controller's `MerkleTree` (`account_sdk/.../session/
/// merkle.rs`), substituting Poseidon → blake2b_256. The off-chain root builder
/// MUST use the identical scheme (leaf, sort, pad-with-zero for odd levels).
fn merkle_verify(root: &[u8; 32], leaf: &[u8; 32], proof: &[u8]) -> bool {
    if proof.len() % 32 != 0 {
        return false;
    }
    let mut node = *leaf;
    for chunk in proof.chunks_exact(32) {
        let sibling: [u8; 32] = chunk.try_into().unwrap();
        node = hash_pair(&node, &sibling);
    }
    &node == root
}

/// blake2b_256 of the two 32-byte nodes in ascending byte order (sorted pair).
fn hash_pair(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut buf = [0u8; 64];
    if a <= b {
        buf[0..32].copy_from_slice(a);
        buf[32..64].copy_from_slice(b);
    } else {
        buf[0..32].copy_from_slice(b);
        buf[32..64].copy_from_slice(a);
    }
    blake2b_256(buf)
}

#[cfg(test)]
mod merkle_tests {
    use super::*;

    // A 3-leaf tree, zero-padded to 4, built with the same sorted-pair scheme:
    //   level0: [l0, l1, l2, ZERO]
    //   level1: [hp(l0,l1), hp(l2,ZERO)]
    //   root:   hp(level1[0], level1[1])
    fn fixture() -> ([u8; 32], [u8; 32], [u8; 32], [u8; 32]) {
        let l0 = [1u8; 32];
        let l1 = [2u8; 32];
        let l2 = [3u8; 32];
        let zero = [0u8; 32];
        let n0 = hash_pair(&l0, &l1);
        let n1 = hash_pair(&l2, &zero);
        let root = hash_pair(&n0, &n1);
        (l0, n1, root, l1)
    }

    #[test]
    fn hash_pair_is_order_independent() {
        let a = [7u8; 32];
        let b = [9u8; 32];
        assert_eq!(hash_pair(&a, &b), hash_pair(&b, &a));
    }

    #[test]
    fn verifies_valid_proof() {
        let (l0, n1, root, l1) = fixture();
        // proof for l0: sibling l1 (level0), then n1 (level1)
        let mut proof = [0u8; 64];
        proof[0..32].copy_from_slice(&l1);
        proof[32..64].copy_from_slice(&n1);
        assert!(merkle_verify(&root, &l0, &proof));
    }

    #[test]
    fn rejects_corrupted_proof() {
        let (l0, n1, root, _l1) = fixture();
        let mut proof = [0u8; 64];
        proof[0..32].copy_from_slice(&[0xAAu8; 32]); // wrong sibling
        proof[32..64].copy_from_slice(&n1);
        assert!(!merkle_verify(&root, &l0, &proof));
    }

    #[test]
    fn single_leaf_root_is_leaf() {
        let leaf = [5u8; 32];
        assert!(merkle_verify(&leaf, &leaf, &[]));
    }

    #[test]
    fn rejects_misaligned_proof() {
        let leaf = [5u8; 32];
        assert!(!merkle_verify(&leaf, &leaf, &[0u8; 31]));
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    fn sample_params_block() -> [u8; SESSION_PARAMS_LEN] {
        let mut b = [0u8; SESSION_PARAMS_LEN];
        b[SP_SESSION_HASH].copy_from_slice(&[0xABu8; 20]);
        b[SP_EXPIRES_AT].copy_from_slice(&1_700_000_000u64.to_le_bytes());
        b[SP_POLICIES_ROOT].copy_from_slice(&[0xCDu8; 32]);
        b[SP_SPEND_CAP].copy_from_slice(&123_456_789u128.to_le_bytes());
        b[SP_GUARDIAN_HASH].copy_from_slice(&[0xEFu8; 20]);
        b
    }

    #[test]
    fn parses_params() {
        let p = parse_session_params(&sample_params_block());
        assert_eq!(p.session_pubkey_hash, [0xABu8; 20]);
        assert_eq!(p.expires_at, 1_700_000_000);
        assert_eq!(p.policies_root, [0xCDu8; 32]);
        assert_eq!(p.spend_cap, 123_456_789);
        assert_eq!(p.guardian_pubkey_hash, [0xEFu8; 20]);
    }

    #[test]
    fn registered_args_tail_equals_params_block() {
        // In the registered model, the session params equal args[20..96]; parsing
        // either way must agree.
        let pb = sample_params_block();
        let mut args = [0u8; ARGS_LEN];
        args[OWNER_HASH].copy_from_slice(&[0x11u8; 20]);
        args[OWNER_ONLY_ARGS_LEN..ARGS_LEN].copy_from_slice(&pb);
        let a = parse_session_params(&args[OWNER_ONLY_ARGS_LEN..ARGS_LEN]);
        let b = parse_session_params(&pb);
        assert_eq!(a.session_pubkey_hash, b.session_pubkey_hash);
        assert_eq!(a.expires_at, b.expires_at);
        assert_eq!(a.policies_root, b.policies_root);
        assert_eq!(a.spend_cap, b.spend_cap);
    }

    #[test]
    fn auth_message_deterministic_and_bound() {
        let sh = [0x22u8; 32];
        let b1 = sample_params_block();
        // deterministic
        assert_eq!(
            session_auth_message(&sh, 0, &b1),
            session_auth_message(&sh, 0, &b1)
        );
        // param-sensitive: perturbing the spend cap changes the message
        let mut b2 = sample_params_block();
        b2[SP_SPEND_CAP][0] ^= 1;
        assert_ne!(
            session_auth_message(&sh, 0, &b1),
            session_auth_message(&sh, 0, &b2)
        );
        // account-bound: a different script hash changes the message
        let sh2 = [0x33u8; 32];
        assert_ne!(
            session_auth_message(&sh, 0, &b1),
            session_auth_message(&sh2, 0, &b1)
        );
        // epoch-bound (revocation): a different epoch changes the message, so an
        // authorization signed for epoch N is invalid once the account reaches N+1.
        assert_ne!(
            session_auth_message(&sh, 0, &b1),
            session_auth_message(&sh, 1, &b1)
        );
    }
}
