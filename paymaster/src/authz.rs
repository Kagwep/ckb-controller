//! Capability gate for the paymaster.
//!
//! A **sponsor token** is a signed, scoped, expiring capability: the relayer
//! sponsors a transaction only if the client presents a token that (a) is signed
//! by the relayer's trusted authority key, (b) carries the required scope, and
//! (c) has not expired. This is the essence of the Biscuit-gating Fiber applies
//! to its RPC (`fiber/crates/fiber-lib/src/rpc/biscuit.rs`): an offline-verifiable
//! bearer capability. The [`Gate`] trait lets production swap this minimal
//! Ed25519 implementation for full `biscuit-auth` (datalog policies, attenuation)
//! without changing the relay logic.
//!
//! Token wire format: `canonical(capability) ‖ ed25519_signature(64)`, where
//! `canonical = scope_len(2 LE) ‖ scope ‖ subject_len(2 LE) ‖ subject ‖
//! not_after(8 LE)`.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

const SIG_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    /// What the token authorizes, e.g. "ckb-controller-sponsor".
    pub scope: String,
    /// Who it is for (player id / account), for rate limiting and audit.
    pub subject: String,
    /// Unix seconds after which the token is invalid.
    pub not_after: u64,
}

impl Capability {
    fn canonical(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&(self.scope.len() as u16).to_le_bytes());
        b.extend_from_slice(self.scope.as_bytes());
        b.extend_from_slice(&(self.subject.len() as u16).to_le_bytes());
        b.extend_from_slice(self.subject.as_bytes());
        b.extend_from_slice(&self.not_after.to_le_bytes());
        b
    }

    /// Parse the canonical prefix of `token`, returning the capability and the
    /// number of bytes consumed (where the signature begins).
    fn parse(token: &[u8]) -> Result<(Capability, usize), AuthzError> {
        let mut i = 0usize;
        let take = |buf: &[u8], i: &mut usize, n: usize| -> Result<(), AuthzError> {
            if *i + n > buf.len() {
                return Err(AuthzError::Malformed);
            }
            *i += n;
            Ok(())
        };
        let read_u16 = |buf: &[u8], i: &mut usize| -> Result<usize, AuthzError> {
            if *i + 2 > buf.len() {
                return Err(AuthzError::Malformed);
            }
            let v = u16::from_le_bytes([buf[*i], buf[*i + 1]]) as usize;
            *i += 2;
            Ok(v)
        };

        let scope_len = read_u16(token, &mut i)?;
        let scope_start = i;
        take(token, &mut i, scope_len)?;
        let scope = String::from_utf8(token[scope_start..i].to_vec())
            .map_err(|_| AuthzError::Malformed)?;

        let subject_len = read_u16(token, &mut i)?;
        let subject_start = i;
        take(token, &mut i, subject_len)?;
        let subject = String::from_utf8(token[subject_start..i].to_vec())
            .map_err(|_| AuthzError::Malformed)?;

        if i + 8 > token.len() {
            return Err(AuthzError::Malformed);
        }
        let not_after = u64::from_le_bytes(token[i..i + 8].try_into().unwrap());
        i += 8;

        Ok((
            Capability {
                scope,
                subject,
                not_after,
            },
            i,
        ))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthzError {
    Malformed,
    BadSignature,
    WrongScope,
    Expired,
    /// The token verified but the authorizer's policy did not grant access
    /// (e.g. a biscuit whose scope/expiry/attenuation checks failed, or a
    /// revoked token). Datalog gates report denial as a single outcome rather
    /// than a typed reason, so the cause is intentionally not leaked.
    Denied,
}

/// Pluggable authorization. Production may implement this over `biscuit-auth`.
pub trait Gate {
    fn authorize(
        &self,
        token: &[u8],
        now: u64,
        required_scope: &str,
    ) -> Result<Capability, AuthzError>;
}

/// Issues sponsor tokens (held by the trusted authority — e.g. the game backend
/// that also blesses on-chain sessions; the same trust root can do both).
pub struct Ed25519Authority {
    key: SigningKey,
}

impl Ed25519Authority {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(seed),
        }
    }

    /// The public key to configure the paymaster's [`Ed25519Gate`] with.
    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    pub fn issue(&self, cap: &Capability) -> Vec<u8> {
        let canonical = cap.canonical();
        let sig: Signature = self.key.sign(&canonical);
        let mut token = canonical;
        token.extend_from_slice(&sig.to_bytes());
        token
    }
}

/// Verifies sponsor tokens against the authority's public key.
pub struct Ed25519Gate {
    public_key: VerifyingKey,
}

impl Ed25519Gate {
    pub fn new(public_key: [u8; 32]) -> Result<Self, AuthzError> {
        Ok(Self {
            public_key: VerifyingKey::from_bytes(&public_key).map_err(|_| AuthzError::BadSignature)?,
        })
    }
}

impl Gate for Ed25519Gate {
    fn authorize(
        &self,
        token: &[u8],
        now: u64,
        required_scope: &str,
    ) -> Result<Capability, AuthzError> {
        let (cap, sig_start) = Capability::parse(token)?;
        let sig_bytes = token.get(sig_start..sig_start + SIG_LEN).ok_or(AuthzError::Malformed)?;
        let sig = Signature::from_bytes(sig_bytes.try_into().map_err(|_| AuthzError::Malformed)?);

        // (a) signed by the trusted authority over the canonical capability.
        self.public_key
            .verify(&token[..sig_start], &sig)
            .map_err(|_| AuthzError::BadSignature)?;
        // (b) correct scope.
        if cap.scope != required_scope {
            return Err(AuthzError::WrongScope);
        }
        // (c) not expired.
        if now > cap.not_after {
            return Err(AuthzError::Expired);
        }
        Ok(cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Ed25519Authority, Ed25519Gate, Capability) {
        let authority = Ed25519Authority::from_seed(&[7u8; 32]);
        let gate = Ed25519Gate::new(authority.public_key()).unwrap();
        let cap = Capability {
            scope: "ckb-controller-sponsor".into(),
            subject: "player-1".into(),
            not_after: 2_000_000_000,
        };
        (authority, gate, cap)
    }

    #[test]
    fn valid_token_authorizes() {
        let (authority, gate, cap) = fixture();
        let token = authority.issue(&cap);
        let got = gate
            .authorize(&token, 1_700_000_000, "ckb-controller-sponsor")
            .unwrap();
        assert_eq!(got, cap);
    }

    #[test]
    fn expired_token_rejected() {
        let (authority, gate, cap) = fixture();
        let token = authority.issue(&cap);
        assert_eq!(
            gate.authorize(&token, cap.not_after + 1, "ckb-controller-sponsor"),
            Err(AuthzError::Expired)
        );
    }

    #[test]
    fn wrong_scope_rejected() {
        let (authority, gate, cap) = fixture();
        let token = authority.issue(&cap);
        assert_eq!(
            gate.authorize(&token, 1_700_000_000, "some-other-scope"),
            Err(AuthzError::WrongScope)
        );
    }

    #[test]
    fn wrong_authority_rejected() {
        let (authority, _gate, cap) = fixture();
        let token = authority.issue(&cap);
        let attacker_gate = Ed25519Gate::new(Ed25519Authority::from_seed(&[9u8; 32]).public_key()).unwrap();
        assert_eq!(
            attacker_gate.authorize(&token, 1_700_000_000, "ckb-controller-sponsor"),
            Err(AuthzError::BadSignature)
        );
    }

    #[test]
    fn tampered_token_rejected() {
        let (authority, gate, cap) = fixture();
        let mut token = authority.issue(&cap);
        token[0] ^= 1; // flip a byte in the scope length / payload
        let r = gate.authorize(&token, 1_700_000_000, "ckb-controller-sponsor");
        assert!(matches!(r, Err(AuthzError::BadSignature) | Err(AuthzError::Malformed)));
    }
}
