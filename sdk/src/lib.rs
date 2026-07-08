//! Off-chain builder SDK for the controller session lock.
//!
//! Everything here mirrors the on-chain layout in
//! `contracts/controller-session-lock/src/main.rs`. The single risk is drift
//! between this and the lock; the `sdk_sanity` integration tests defend against
//! it by running SDK-built transactions through the real lock binary in CKB-VM.
//!
//! Signing is left to the caller (sign the messages this SDK computes, then pass
//! the 65-byte recoverable secp256k1 signatures back into the witness builders),
//! so the SDK stays agnostic to the key-management / signer in use.

use ckb_hash::blake2b_256;
use ckb_types::{
    bytes::Bytes,
    core::TransactionView,
    packed::{Script, WitnessArgs},
    prelude::*,
};

pub mod channel;
pub mod game;

// ---- Layout constants (MUST match the lock) --------------------------------
pub const SIGNATURE_LEN: usize = 65;
pub const MODE_OWNER: u8 = 0;
pub const MODE_SESSION: u8 = 1;
pub const OWNER_HASH_LEN: usize = 20;
pub const SESSION_PARAMS_LEN: usize = 96;
pub const REGISTERED_ARGS_LEN: usize = OWNER_HASH_LEN + SESSION_PARAMS_LEN; // 116
/// Sentinel policies root meaning "any policy allowed".
pub const WILDCARD_ROOT: [u8; 32] = [0xFFu8; 32];
/// Sentinel spend cap meaning "no limit".
pub const SPEND_CAP_UNLIMITED: u128 = u128::MAX;
/// Sentinel expiry meaning "never expires": the lock skips its header-dep read,
/// so the session can authorize a counterparty-built tx with no header dep (e.g.
/// a Fiber funding tx). Bounded exposure still comes from the spend cap.
pub const NO_EXPIRY: u64 = u64::MAX;
/// Domain separator for the carried-model owner authorization.
pub const SESSION_AUTH_DOMAIN: &[u8] = b"ckb-controller/session-auth/v1";

// ---- Args / params / data --------------------------------------------------

/// The 96-byte session-params block:
/// session_hash ‖ expires_at(8 LE) ‖ policies_root ‖ spend_cap(16 LE) ‖ guardian.
pub fn session_params(
    session_pubkey_hash: &[u8; 20],
    expires_at: u64,
    policies_root: &[u8; 32],
    spend_cap: u128,
    guardian_pubkey_hash: &[u8; 20],
) -> Vec<u8> {
    let mut p = Vec::with_capacity(SESSION_PARAMS_LEN);
    p.extend_from_slice(session_pubkey_hash);
    p.extend_from_slice(&expires_at.to_le_bytes());
    p.extend_from_slice(policies_root);
    p.extend_from_slice(&spend_cap.to_le_bytes());
    p.extend_from_slice(guardian_pubkey_hash);
    debug_assert_eq!(p.len(), SESSION_PARAMS_LEN);
    p
}

/// Authorization-carried model args: just the owner pubkey hash (20 bytes).
pub fn owner_only_args(owner_pubkey_hash: &[u8; 20]) -> Bytes {
    owner_pubkey_hash.to_vec().into()
}

/// Registered model args: owner_pubkey_hash ‖ session_params (116 bytes).
pub fn registered_args(owner_pubkey_hash: &[u8; 20], params: &[u8]) -> Bytes {
    let mut a = owner_pubkey_hash.to_vec();
    a.extend_from_slice(params);
    a.into()
}

/// Account cell data encoding a revocation epoch (empty == epoch 0).
pub fn epoch_data(epoch: u64) -> Bytes {
    if epoch == 0 {
        Bytes::new()
    } else {
        epoch.to_le_bytes().to_vec().into()
    }
}

// ---- Messages --------------------------------------------------------------

/// The tx message the session/owner key signs: blake2b_256 of the raw tx with
/// cell_deps cleared (so a paymaster may attach fee cell_deps freely).
pub fn tx_message(tx: &TransactionView) -> [u8; 32] {
    let raw = tx
        .data()
        .raw()
        .as_builder()
        .cell_deps(Default::default())
        .build();
    blake2b_256(raw.as_slice())
}

/// The carried-model owner-authorization message: blake2b_256(domain ‖
/// script_hash ‖ revocation_epoch(8 LE) ‖ params).
pub fn session_auth_message(script_hash: &[u8; 32], revocation_epoch: u64, params: &[u8]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(SESSION_AUTH_DOMAIN.len() + 32 + 8 + params.len());
    buf.extend_from_slice(SESSION_AUTH_DOMAIN);
    buf.extend_from_slice(script_hash);
    buf.extend_from_slice(&revocation_epoch.to_le_bytes());
    buf.extend_from_slice(params);
    blake2b_256(buf)
}

// ---- Policy Merkle tree (sorted-pair blake2b, matches the lock) ------------

/// Policy dimensions (must match the lock). A constrained output is allowed by
/// its TYPE-script hash or its LOCK-script hash.
pub const POLICY_KIND_TYPE: u8 = 0;
pub const POLICY_KIND_LOCK: u8 = 1;

/// Leaf for a policy: blake2b_256(script_hash). The same hashing is used for both
/// type and lock dimensions (the script hash already distinguishes them).
pub fn policy_leaf(script_hash: &[u8; 32]) -> [u8; 32] {
    blake2b_256(script_hash)
}

/// Convenience: policy leaf for a ckb-types `Script` (uses its script hash).
/// Works for both type-script and lock-script policies.
pub fn policy_leaf_for_script(script: &Script) -> [u8; 32] {
    let h: [u8; 32] = script.calc_script_hash().as_slice().try_into().unwrap();
    policy_leaf(&h)
}

/// Back-compat alias for type-script policy leaves.
pub fn policy_leaf_for_type_script(type_script: &Script) -> [u8; 32] {
    policy_leaf_for_script(type_script)
}

fn pair(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
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

fn fold_level(level: &mut Vec<[u8; 32]>) {
    if level.len() % 2 == 1 {
        level.push([0u8; 32]);
    }
    *level = level.chunks(2).map(|c| pair(&c[0], &c[1])).collect();
}

/// Merkle root over the policy leaves (sorted-pair, ZERO-padded odd levels).
/// Empty set => all-zero root.
pub fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut level = leaves.to_vec();
    while level.len() > 1 {
        fold_level(&mut level);
    }
    level[0]
}

/// Membership proof for `leaves[index]`: sibling hashes from leaf to root.
pub fn merkle_proof(leaves: &[[u8; 32]], mut index: usize) -> Vec<[u8; 32]> {
    let mut proof = Vec::new();
    let mut level = leaves.to_vec();
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            level.push([0u8; 32]);
        }
        proof.push(level[index ^ 1]); // sibling
        index >>= 1;
        level = level.chunks(2).map(|c| pair(&c[0], &c[1])).collect();
    }
    proof
}

/// Verify a proof (mirror of the on-chain `merkle_verify`), for SDK self-tests.
pub fn merkle_verify(root: &[u8; 32], leaf: &[u8; 32], proof: &[[u8; 32]]) -> bool {
    let mut node = *leaf;
    for sib in proof {
        node = pair(&node, sib);
    }
    &node == root
}

// ---- Witness assembly ------------------------------------------------------

/// Frame per-output policy proofs into the lock's proof region: for each
/// constrained (non-account) output in output order,
/// `kind(1) ‖ proof_len(1) ‖ siblings(32·n)`, where kind is POLICY_KIND_TYPE or
/// POLICY_KIND_LOCK.
pub fn proof_region(entries: &[(u8, Vec<[u8; 32]>)]) -> Vec<u8> {
    let mut r = Vec::new();
    for (kind, proof) in entries {
        r.push(*kind);
        r.push(proof.len() as u8);
        for sib in proof {
            r.extend_from_slice(sib);
        }
    }
    r
}

fn wrap_lock(lock_bytes: Vec<u8>) -> Bytes {
    WitnessArgs::new_builder()
        .lock(Some(Bytes::from(lock_bytes)).pack())
        .build()
        .as_bytes()
}

/// OWNER-mode witness (`WitnessArgs.lock`).
pub fn owner_witness(owner_sig: &[u8]) -> Bytes {
    let mut b = vec![MODE_OWNER];
    b.extend_from_slice(owner_sig);
    wrap_lock(b)
}

/// SESSION-mode witness, registered model:
/// mode ‖ session_sig ‖ [guardian_sig] ‖ proof_region.
pub fn session_witness_registered(
    session_sig: &[u8],
    guardian_sig: Option<&[u8]>,
    proof_region: &[u8],
) -> Bytes {
    let mut b = vec![MODE_SESSION];
    b.extend_from_slice(session_sig);
    if let Some(g) = guardian_sig {
        b.extend_from_slice(g);
    }
    b.extend_from_slice(proof_region);
    wrap_lock(b)
}

/// SESSION-mode witness, authorization-carried model:
/// mode ‖ params(96) ‖ owner_auth(65) ‖ session_sig ‖ [guardian_sig] ‖ proofs.
pub fn session_witness_carried(
    params: &[u8],
    owner_auth: &[u8],
    session_sig: &[u8],
    guardian_sig: Option<&[u8]>,
    proof_region: &[u8],
) -> Bytes {
    let mut b = vec![MODE_SESSION];
    b.extend_from_slice(params);
    b.extend_from_slice(owner_auth);
    b.extend_from_slice(session_sig);
    if let Some(g) = guardian_sig {
        b.extend_from_slice(g);
    }
    b.extend_from_slice(proof_region);
    wrap_lock(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_length() {
        let p = session_params(&[1; 20], 42, &WILDCARD_ROOT, 7, &[0; 20]);
        assert_eq!(p.len(), SESSION_PARAMS_LEN);
        assert_eq!(registered_args(&[9; 20], &p).len(), REGISTERED_ARGS_LEN);
    }

    #[test]
    fn merkle_roundtrip_various_sizes() {
        for n in 1..=8usize {
            let leaves: Vec<[u8; 32]> = (0..n).map(|i| [i as u8; 32]).collect();
            let root = merkle_root(&leaves);
            for (i, leaf) in leaves.iter().enumerate() {
                let proof = merkle_proof(&leaves, i);
                assert!(merkle_verify(&root, leaf, &proof), "n={n} i={i}");
            }
        }
    }

    #[test]
    fn single_leaf_root_is_leaf() {
        let leaf = [5u8; 32];
        assert_eq!(merkle_root(&[leaf]), leaf);
        assert!(merkle_verify(&leaf, &leaf, &[]));
    }

    #[test]
    fn auth_message_epoch_sensitive() {
        let sh = [2u8; 32];
        let p = session_params(&[1; 20], 1, &WILDCARD_ROOT, SPEND_CAP_UNLIMITED, &[0; 20]);
        assert_ne!(
            session_auth_message(&sh, 0, &p),
            session_auth_message(&sh, 1, &p)
        );
    }
}
