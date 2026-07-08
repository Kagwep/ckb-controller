//! Standard `secp256k1_blake160_sighash_all` signing — the operator's own lock
//! signature on a game-cell transition (the `finalize` seam of [`crate::GameOperator`]).
//!
//! The transition tx's witness 0 already carries the intent batch in `input_type`;
//! this fills the same witness's `lock` field with the 65-byte recoverable
//! signature the system sighash lock expects, leaving everything else untouched
//! (the signer may only fill witnesses — the same rule Fiber's external funding
//! imposes). Message = blake2b_256(tx_hash ‖ len(w0) ‖ w0-with-zeroed-lock ‖
//! [len(w) ‖ w for each witness beyond the inputs]), per the system script.

use anyhow::{anyhow, Result};
use ckb_hash::new_blake2b;
use ckb_types::{bytes::Bytes, core::TransactionView, packed::WitnessArgs, prelude::*};
use secp256k1::{Message, Secp256k1, SecretKey};

/// Sign witness 0 for the standard sighash-all lock, preserving its
/// `input_type`/`output_type` fields. Assumes the single-lock-group shape
/// [`crate::GameOperator::build_transition`] emits (input 0 is the only cell
/// under this lock).
pub fn sign_sighash_all(tx: TransactionView, sk: &SecretKey) -> Result<TransactionView> {
    let witness0 = tx
        .witnesses()
        .get(0)
        .ok_or_else(|| anyhow!("tx has no witness 0"))?;
    let args = WitnessArgs::from_slice(&witness0.raw_data())
        .map_err(|e| anyhow!("witness 0 is not WitnessArgs: {e}"))?;

    let sig65 = {
        let mut msg = [0u8; 32];
        let mut hasher = new_blake2b();
        hasher.update(tx.hash().as_slice());
        hash_witness(&mut hasher, &zeroed_lock_witness(&args));
        for w in tx.witnesses().into_iter().skip(tx.inputs().len()) {
            hash_witness(&mut hasher, &w.raw_data());
        }
        hasher.finalize(&mut msg);

        let sig = Secp256k1::new().sign_ecdsa_recoverable(&Message::from_slice(&msg)?, sk);
        let (rec_id, compact) = sig.serialize_compact();
        let mut out = [0u8; 65];
        out[..64].copy_from_slice(&compact);
        out[64] = rec_id.to_i32() as u8;
        out
    };

    let signed = args
        .as_builder()
        .lock(Some(Bytes::from(sig65.to_vec())).pack())
        .build();
    let mut witnesses: Vec<_> = tx.witnesses().into_iter().collect();
    witnesses[0] = signed.as_bytes().pack();
    Ok(tx.as_advanced_builder().set_witnesses(witnesses).build())
}

/// The witness as hashed for signing: `lock` filled with a 65-byte placeholder.
fn zeroed_lock_witness(args: &WitnessArgs) -> Vec<u8> {
    args.clone()
        .as_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])).pack())
        .build()
        .as_bytes()
        .to_vec()
}

fn hash_witness(hasher: &mut ckb_hash::Blake2b, witness: &[u8]) {
    hasher.update(&(witness.len() as u64).to_le_bytes());
    hasher.update(witness);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::{
        core::TransactionBuilder,
        packed::{CellInput, CellOutput, OutPoint, Script},
    };
    use secp256k1::ecdsa::{RecoverableSignature, RecoveryId};

    fn dummy_tx() -> TransactionView {
        let batch = Bytes::from(vec![7u8; 55]); // stands in for an intent batch
        let witness = WitnessArgs::new_builder()
            .input_type(Some(batch).pack())
            .build()
            .as_bytes();
        TransactionBuilder::default()
            .input(
                CellInput::new_builder()
                    .previous_output(OutPoint::new(Default::default(), 0))
                    .build(),
            )
            .output(CellOutput::new_builder().capacity(500u64.pack()).lock(Script::default()).build())
            .output_data(Bytes::from(vec![1, 2, 3]).pack())
            .witness(witness.pack())
            .build()
    }

    #[test]
    fn signature_recovers_to_the_signer_and_preserves_batch() {
        let sk = SecretKey::from_slice(&[0x11u8; 32]).unwrap();
        let tx = dummy_tx();
        let signed = sign_sighash_all(tx.clone(), &sk).unwrap();

        // structure untouched: same hash (witnesses aren't part of tx_hash), same batch.
        assert_eq!(signed.hash(), tx.hash());
        let args = WitnessArgs::from_slice(&signed.witnesses().get(0).unwrap().raw_data()).unwrap();
        let batch: Bytes = args.input_type().to_opt().unwrap().unpack();
        assert_eq!(batch.as_ref(), &[7u8; 55][..]);

        // recompute the message exactly as the system lock does and recover.
        let sig: Bytes = args.lock().to_opt().unwrap().unpack();
        let mut msg = [0u8; 32];
        let mut hasher = new_blake2b();
        hasher.update(signed.hash().as_slice());
        let orig = WitnessArgs::from_slice(&tx.witnesses().get(0).unwrap().raw_data()).unwrap();
        hash_witness(&mut hasher, &zeroed_lock_witness(&orig));
        hasher.finalize(&mut msg);

        let rec = RecoverableSignature::from_compact(
            &sig[..64],
            RecoveryId::from_i32(sig[64] as i32).unwrap(),
        )
        .unwrap();
        let pk = Secp256k1::new()
            .recover_ecdsa(&Message::from_slice(&msg).unwrap(), &rec)
            .unwrap();
        assert_eq!(
            pk,
            secp256k1::PublicKey::from_secret_key(&Secp256k1::new(), &sk)
        );
    }
}
