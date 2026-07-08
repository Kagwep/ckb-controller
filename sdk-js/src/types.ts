// Shared shapes: the config pair (controller.config.json + manifest) and the
// slice of the controller-wasm module the SDK calls. The wasm module is
// INJECTED by the caller (already initialised) — the SDK does no wasm loading,
// so it works identically under Vite, Node, or any bundler.
//
// Wire formats behind all of this: docs/internals/wire-formats.md.

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

/** One network's entries in .controller/manifest.json. */
export type NetworkManifest = Record<string, Artifact>;

/** controller.config.json. */
export interface ControllerConfig {
  network: string;
  keyFile?: string;
  gameId: string;
  session: {
    ownerPrivkey: string;
    sessionPrivkey: string;
    spendCapCkb: number | string;
    expiresAt: "never" | number | string;
    policiesRoot: "wildcard" | string;
    guardian: string | null;
  };
  operator?: { listen?: string; chain?: string; feeShannons?: number };
  fiber?: { rpc?: string };
  networks: Record<string, { rpc: string; explorerTx?: string; [k: string]: unknown }>;
}

/** A CKB JSON-RPC Script (snake_case — the shape fiber-js and node RPCs use). */
export interface JsonScript {
  code_hash: string;
  hash_type: "data" | "type" | "data1" | "data2";
  args: string;
}

/** The controller-wasm surface the SDK uses (pass the initialised module). */
export interface ControllerWasm {
  session_params(sessionHash: string, expiresAt: bigint, root: string, spendCapShannons: string, guardian: string): string;
  channel_session_params(sessionHash: string, expiresAt: bigint, fundingLockMol: string, spendCapShannons: bigint, guardian: string): string;
  registered_args(ownerHash: string, params: string): string;
  controller_address(codeHash: string, hashType: number, args: string, testnet: boolean): string;
  script(codeHash: string, hashType: number, args: string): string;
  no_expiry(): bigint;
  wildcard_root(): string;
  tx_message(txMoleculeHex: string): string;
  session_witness_registered(sessionSig: string, guardianSig: string, proofRegion: string): string;
  channel_proof_region(): string;
  game_intent_message(gameId: string, playerHash: string, points: bigint, nonce: bigint): string;
  game_encode_intent(playerHash: string, points: bigint, nonce: bigint, sig: string): string;
  game_decode_state(dataHex: string): string;
  ChannelSession: new (accountLockMol: string, accountInput: string, accountCapacity: bigint, fundingLockMol: string, headerDep: string) => WasmChannelSession;
}

export interface WasmChannelSession {
  open(peer: string, budgetShannons: bigint): { outpoint: string; message: string };
  pay(amountShannons: bigint): void;
  spent(): string;
  remaining(): string;
  is_open(): boolean;
  close(): { local: string; remote: string; settle: { outpoint: string; message: string } };
}

export const CKB = 100_000_000n; // shannons per CKB
