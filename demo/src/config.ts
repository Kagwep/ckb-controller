// Project config + deployment manifest — the browser-side counterpart of
// ../controller-config.mjs. Both JSON files live at the REPO root and are
// statically imported (bundled at build time); vite.config.ts allows the
// parent-dir read in dev. Wire formats: ../../docs/internals/wire-formats.md.
import config from "../../controller.config.json";
import manifest from "../../.controller/manifest.json";

export interface DepPoint {
  txHash: string;
  index: string;
}
export interface Artifact {
  codeHash?: string;
  hashType?: string;
  dep: DepPoint;
  depType?: string;
}

export const CONFIG = config;
export const MANIFEST = manifest as unknown as Record<string, Record<string, Artifact>>;
export const NETWORK = config.network;
export const NET = MANIFEST[NETWORK] ?? {};
export const RPC = (config.networks as Record<string, { rpc: string; explorerTx: string }>)[NETWORK].rpc;

export function artifact(name: string): Artifact {
  const a = NET[name];
  if (!a) throw new Error(`manifest has no "${name}" for network "${NETWORK}"`);
  return a;
}

const hexBytes = (h: string): Uint8Array =>
  Uint8Array.from((h.replace(/^0x/, "").match(/.{2}/g) ?? []).map((b) => parseInt(b, 16)));

// FIXED DEMO KEYS from config (testnet only, publicly known).
export const OWNER_PRIV = hexBytes(config.session.ownerPrivkey);
export const SESSION_PRIV = hexBytes(config.session.sessionPrivkey);
export const SPEND_CAP_SHANNONS = (BigInt(config.session.spendCapCkb) * 100000000n).toString();
