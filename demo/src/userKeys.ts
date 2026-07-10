// Per-browser controller identity (Phase 1, multi-user mode only). Instead of
// the shared fixed demo keys (config.ts OWNER_PRIV/SESSION_PRIV), each browser
// generates its own owner+session keypair once and reuses it — so two browsers
// derive two distinct accounts/locks/addresses and fund independent channels.
//
// Storage: private keys as hex in localStorage under STORE_KEY. Testnet-only,
// low-value demo material — NOT production key management (passkey unlock is a
// later phase). Reset by clearing the key or calling resetUserKeys().
import { genKey, keyFromHex, hx, type UserKeys } from "@ckb-controller/sdk";

const STORE_KEY = "ckb-controller.userKeys.v1";

interface StoredKeys {
  owner: string;
  session: string;
}

/** The per-browser keypair, generated + persisted on first call, reused after. */
export function getUserKeys(): UserKeys {
  const raw = localStorage.getItem(STORE_KEY);
  if (raw) {
    const s = JSON.parse(raw) as StoredKeys;
    return { owner: keyFromHex(s.owner), session: keyFromHex(s.session) };
  }
  const keys: UserKeys = { owner: genKey(), session: genKey() };
  const stored: StoredKeys = { owner: hx(keys.owner.priv), session: hx(keys.session.priv) };
  localStorage.setItem(STORE_KEY, JSON.stringify(stored));
  return keys;
}

/** Forget this browser's identity (next getUserKeys() mints a fresh one). */
export function resetUserKeys(): void {
  localStorage.removeItem(STORE_KEY);
}
