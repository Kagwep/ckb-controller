// Key material + signatures, exactly as the lock (via ckb-auth) expects:
// blake160 pubkey hash = blake2b-256("ckb-default-hash")[0..20] of the
// compressed pubkey; signature = 65-byte recoverable secp256k1 (r ‖ s ‖ recid).
import { secp256k1 } from "@noble/curves/secp256k1";
import { blake2b } from "@noble/hashes/blake2b";
import { bytesToHex, hexToBytes, utf8ToBytes } from "@noble/hashes/utils";

const CKB_HASH_PERSONAL = utf8ToBytes("ckb-default-hash");

export const hx = (b: Uint8Array): string => "0x" + bytesToHex(b);
export const strip = (h: string): string => (h.startsWith("0x") ? h.slice(2) : h);
export const hexBytes = (h: string): Uint8Array => hexToBytes(strip(h));

export function ckbHash(data: Uint8Array): Uint8Array {
  return blake2b(data, { dkLen: 32, personalization: CKB_HASH_PERSONAL });
}

/** 0x + 20-byte blake160 of a private key's compressed pubkey. */
export function pubHash(priv: Uint8Array): string {
  return hx(ckbHash(secp256k1.getPublicKey(priv, true)).slice(0, 20));
}

export interface KeyPair {
  priv: Uint8Array;
  pubHash: string;
}

export function genKey(): KeyPair {
  const priv = secp256k1.utils.randomPrivateKey();
  return { priv, pubHash: pubHash(priv) };
}

export function keyFromHex(privHex: string): KeyPair {
  const priv = hexBytes(privHex);
  return { priv, pubHash: pubHash(priv) };
}

/** 65-byte recoverable signature over a 0x-hex 32-byte message. */
export function signRecoverable(msgHex: string, priv: Uint8Array): string {
  const sig = secp256k1.sign(hexBytes(msgHex), priv);
  const out = new Uint8Array(65);
  out.set(sig.toCompactRawBytes(), 0);
  out[64] = sig.recovery;
  return hx(out);
}
