//! Production capability gate over [`biscuit-auth`].
//!
//! [`authz::Ed25519Gate`](crate::authz::Ed25519Gate) is a minimal, hand-rolled
//! bearer token — fine as a reference, but not what Fiber runs. Fiber gates its
//! RPC with **biscuit** (`fiber/crates/fiber-lib/src/rpc/biscuit.rs`): tokens are
//! offline-verifiable datalog capabilities that the holder can *attenuate*
//! (append-only restrictions) without contacting the issuer, and that the
//! verifier can *revoke* by id. This module gives the paymaster the same gate,
//! behind the same [`authz::Gate`] trait, so the relay logic is unchanged.
//!
//! ## Token shape
//!
//! The authority ([`BiscuitAuthority`]) signs an authority block carrying three
//! facts and one check:
//!
//! ```datalog
//! sponsor("ckb-controller-sponsor");   // the capability's scope
//! subject("player-1");                 // who it is for (rate limiting / audit)
//! expires(2033-05-18T03:33:20Z);       // a Date; not_after as a fact ...
//! check if expires($e), time($t), $t <= $e;   // ... enforced here, so even an
//!                                              // attenuated token can't outlive it
//! ```
//!
//! The gate ([`BiscuitGate`]) verifies the signature against the authority's
//! public key, rejects revoked tokens, then runs an authorizer that supplies the
//! current `time` and the policy `allow if sponsor($scope)` for the required
//! scope. Verified facts are read back out as a [`Capability`] for audit.

use std::collections::HashSet;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use biscuit_auth::{
    builder::{Fact, Term},
    Algorithm, AuthorizerBuilder, Biscuit, KeyPair, PublicKey,
};

use crate::authz::{AuthzError, Capability, Gate};

const SCOPE_FACT: &str = "sponsor";
const SUBJECT_FACT: &str = "subject";
const EXPIRES_FACT: &str = "expires";

fn to_systemtime(unix_secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(unix_secs)
}

/// Issues biscuit sponsor tokens. Held by the relayer's trusted authority — the
/// same trust root that blesses on-chain sessions can mint these.
pub struct BiscuitAuthority {
    root: KeyPair,
}

impl BiscuitAuthority {
    /// Deterministic authority from a 32-byte seed (handy for tests / config).
    pub fn from_seed(seed: &[u8; 32]) -> Result<Self, AuthzError> {
        let root =
            KeyPair::from_bytes(seed, Algorithm::Ed25519.into()).map_err(|_| AuthzError::Malformed)?;
        Ok(Self { root })
    }

    /// A fresh random authority (mirrors Fiber's `KeyPair::new`).
    pub fn generate() -> Self {
        Self { root: KeyPair::new() }
    }

    /// The public key to configure the [`BiscuitGate`] with, in biscuit's
    /// canonical `ed25519/<hex>` form (the same string Fiber accepts).
    pub fn public_key(&self) -> String {
        self.root.public().to_string()
    }

    /// Mint a sponsor token encoding `cap`. Returns a base64 biscuit string.
    pub fn issue(&self, cap: &Capability) -> Result<String, AuthzError> {
        let token = Biscuit::builder()
            .fact(Fact::new(
                SCOPE_FACT.to_string(),
                vec![Term::Str(cap.scope.clone())],
            ))
            .and_then(|b| {
                b.fact(Fact::new(
                    SUBJECT_FACT.to_string(),
                    vec![Term::Str(cap.subject.clone())],
                ))
            })
            .and_then(|b| {
                b.fact(Fact::new(
                    EXPIRES_FACT.to_string(),
                    vec![Term::from(to_systemtime(cap.not_after))],
                ))
            })
            .and_then(|b| b.check("check if expires($e), time($t), $t <= $e"))
            .and_then(|b| b.build(&self.root))
            .map_err(|_| AuthzError::Malformed)?;
        token.to_base64().map_err(|_| AuthzError::Malformed)
    }
}

/// Verifies sponsor tokens against the authority's public key, with an optional
/// revocation set.
pub struct BiscuitGate {
    root_pubkey: PublicKey,
    revoked: HashSet<Vec<u8>>,
}

impl BiscuitGate {
    /// Configure from the authority's `ed25519/<hex>` public key string.
    pub fn new(public_key: &str) -> Result<Self, AuthzError> {
        Ok(Self {
            root_pubkey: PublicKey::from_str(public_key).map_err(|_| AuthzError::BadSignature)?,
            revoked: HashSet::new(),
        })
    }

    /// Revoke tokens by their hex revocation id (as Fiber's `extend_revocation_list`
    /// does). A matching token is thereafter [`AuthzError::Denied`].
    pub fn revoke_hex(&mut self, revocation_id: &str) -> Result<(), AuthzError> {
        let id = decode_hex(revocation_id.trim_start_matches("0x")).ok_or(AuthzError::Malformed)?;
        self.revoked.insert(id);
        Ok(())
    }
}

impl Gate for BiscuitGate {
    fn authorize(
        &self,
        token: &[u8],
        now: u64,
        required_scope: &str,
    ) -> Result<Capability, AuthzError> {
        let token_str = std::str::from_utf8(token).map_err(|_| AuthzError::Malformed)?;

        // (a) signature: the token (and any attenuating blocks) must chain back
        //     to the authority's root key.
        let biscuit =
            Biscuit::from_base64(token_str, self.root_pubkey).map_err(|_| AuthzError::BadSignature)?;

        // (b) revocation: reject if any block id is on the revocation list.
        if biscuit
            .revocation_identifiers()
            .iter()
            .any(|id| self.revoked.contains(id))
        {
            return Err(AuthzError::Denied);
        }

        // (c) policy: supply the current time and require the sponsor scope. The
        //     token's own `expires` check + any attenuation are evaluated here too.
        let mut authorizer = AuthorizerBuilder::new()
            .fact(Fact::new(
                "time".to_string(),
                vec![Term::from(to_systemtime(now))],
            ))
            .and_then(|a| {
                a.code_with_params(
                    // `{scope}` is a biscuit parameter (substituted below); a bare
                    // `$scope` would be a datalog *variable* that matches any
                    // sponsor fact — i.e. no scope check at all.
                    "allow if sponsor({scope});",
                    [("scope".to_string(), Term::Str(required_scope.to_string()))].into(),
                    Default::default(),
                )
            })
            .and_then(|a| a.build(&biscuit))
            .map_err(|_| AuthzError::Denied)?;
        authorizer.authorize().map_err(|_| AuthzError::Denied)?;

        // Read the verified facts back out for audit / rate limiting.
        let (scope,): (String,) = authorizer
            .query_exactly_one(format!("data($s) <- {SCOPE_FACT}($s)").as_str())
            .map_err(|_| AuthzError::Malformed)?;
        let (subject,): (String,) = authorizer
            .query_exactly_one(format!("data($s) <- {SUBJECT_FACT}($s)").as_str())
            .map_err(|_| AuthzError::Malformed)?;
        let (expires,): (SystemTime,) = authorizer
            .query_exactly_one(format!("data($e) <- {EXPIRES_FACT}($e)").as_str())
            .map_err(|_| AuthzError::Malformed)?;
        let not_after = expires
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AuthzError::Malformed)?
            .as_secs();

        Ok(Capability {
            scope,
            subject,
            not_after,
        })
    }
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOPE: &str = "ckb-controller-sponsor";
    const NOW: u64 = 1_700_000_000;
    const FAR_FUTURE: u64 = 2_000_000_000;

    fn cap(not_after: u64) -> Capability {
        Capability {
            scope: SCOPE.into(),
            subject: "player-1".into(),
            not_after,
        }
    }

    fn fixture() -> (BiscuitAuthority, BiscuitGate) {
        let authority = BiscuitAuthority::from_seed(&[7u8; 32]).unwrap();
        let gate = BiscuitGate::new(&authority.public_key()).unwrap();
        (authority, gate)
    }

    #[test]
    fn valid_token_authorizes() {
        let (authority, gate) = fixture();
        let token = authority.issue(&cap(FAR_FUTURE)).unwrap();
        let got = gate.authorize(token.as_bytes(), NOW, SCOPE).unwrap();
        assert_eq!(got, cap(FAR_FUTURE));
    }

    #[test]
    fn expired_token_denied() {
        let (authority, gate) = fixture();
        let token = authority.issue(&cap(NOW - 1)).unwrap();
        assert_eq!(
            gate.authorize(token.as_bytes(), NOW, SCOPE),
            Err(AuthzError::Denied)
        );
    }

    #[test]
    fn wrong_scope_denied() {
        let (authority, gate) = fixture();
        let token = authority.issue(&cap(FAR_FUTURE)).unwrap();
        assert_eq!(
            gate.authorize(token.as_bytes(), NOW, "some-other-scope"),
            Err(AuthzError::Denied)
        );
    }

    #[test]
    fn wrong_authority_rejected() {
        let (authority, _gate) = fixture();
        let token = authority.issue(&cap(FAR_FUTURE)).unwrap();
        let attacker_gate =
            BiscuitGate::new(&BiscuitAuthority::from_seed(&[9u8; 32]).unwrap().public_key()).unwrap();
        assert_eq!(
            attacker_gate.authorize(token.as_bytes(), NOW, SCOPE),
            Err(AuthzError::BadSignature)
        );
    }

    #[test]
    fn tampered_token_rejected() {
        let (authority, gate) = fixture();
        let mut token = authority.issue(&cap(FAR_FUTURE)).unwrap().into_bytes();
        let mid = token.len() / 2;
        token[mid] ^= 1;
        assert!(matches!(
            gate.authorize(&token, NOW, SCOPE),
            Err(AuthzError::BadSignature) | Err(AuthzError::Malformed)
        ));
    }

    #[test]
    fn revoked_token_denied() {
        let (authority, mut gate) = fixture();
        let token = authority.issue(&cap(FAR_FUTURE)).unwrap();
        // token is good before revocation ...
        assert!(gate.authorize(token.as_bytes(), NOW, SCOPE).is_ok());
        // ... and Denied after its id is revoked.
        let biscuit = Biscuit::from_base64(&token, gate.root_pubkey).unwrap();
        let rev_id = hex_encode(&biscuit.revocation_identifiers()[0]);
        gate.revoke_hex(&rev_id).unwrap();
        assert_eq!(
            gate.authorize(token.as_bytes(), NOW, SCOPE),
            Err(AuthzError::Denied)
        );
    }

    /// Biscuit's headline feature: the *holder* can restrict a token offline
    /// (no issuer round-trip). Here a broad token is attenuated to a tighter
    /// expiry; the gate then honours the stricter caveat.
    #[test]
    fn attenuated_token_honours_added_caveat() {
        let (authority, gate) = fixture();
        let token = authority.issue(&cap(FAR_FUTURE)).unwrap();
        let biscuit = Biscuit::from_base64(&token, gate.root_pubkey).unwrap();

        // holder appends a block: this token now also expires at NOW + 100.
        let attenuated = biscuit
            .append(
                biscuit_auth::builder::BlockBuilder::new()
                    .check(format!("check if time($t), $t <= {}", rfc3339(NOW + 100)).as_str())
                    .unwrap(),
            )
            .unwrap()
            .to_base64()
            .unwrap();

        // within the narrowed window: ok.
        assert!(gate.authorize(attenuated.as_bytes(), NOW + 50, SCOPE).is_ok());
        // past the narrowed window (but before the original FAR_FUTURE): denied.
        assert_eq!(
            gate.authorize(attenuated.as_bytes(), NOW + 200, SCOPE),
            Err(AuthzError::Denied)
        );
    }

    fn hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn rfc3339(unix_secs: u64) -> String {
        // biscuit's datalog Date literals are RFC3339; format minimally in UTC.
        let days = unix_secs / 86400;
        let secs_of_day = unix_secs % 86400;
        let (h, m, s) = (secs_of_day / 3600, (secs_of_day % 3600) / 60, secs_of_day % 60);
        let (y, mo, d) = civil_from_days(days as i64);
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
    }

    // days since 1970-01-01 -> (year, month, day); Howard Hinnant's algorithm.
    fn civil_from_days(z: i64) -> (i64, u32, u32) {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = (z - era * 146097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (if m <= 2 { y + 1 } else { y }, m, d)
    }
}
