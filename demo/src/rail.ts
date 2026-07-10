// Demo adapter over @ckb-controller/sdk's channel rails: the UI keeps its
// createRail(params, lockCodeHash, budget) entry point; everything else —
// MockRail, LiveRail, session signing, the funding-tx red-herring handling —
// now lives in the SDK (this demo is its first consumer).
import init, * as wasmModule from "../pkg/controller.js";
import {
  CKB,
  Controller,
  MockRail,
  type ChannelRail,
  type ControllerConfig,
  type ControllerWasm,
  type PeerConfig,
} from "@ckb-controller/sdk";
import { CONFIG, MANIFEST } from "./config.js";
import { getUserKeys } from "./userKeys.js";

export type DemoRail = ChannelRail;
export { CKB };

/** Multi-user mode (?multi=1): each browser is its own on-chain identity. */
export const isMulti = (params: URLSearchParams): boolean => params.get("multi") === "1";

// In multi-user mode Controller.load derives the account from THIS browser's
// persisted keypair; otherwise the shared fixed demo keys in config are used.
const loadController = (params: URLSearchParams, wasm: ControllerWasm): Controller =>
  Controller.load({
    config: CONFIG as unknown as ControllerConfig,
    manifest: MANIFEST,
    wasm,
    keys: isMulti(params) ? getUserKeys() : undefined,
  });

/** Read peer config from the URL (?peer=…&wss=…&config=…) for live mode. */
export function peerFromUrl(params: URLSearchParams): PeerConfig {
  const peerPubkey = params.get("peer") ?? "";
  const peerWssAddr = params.get("wss") ?? "";
  const configUrl = params.get("config") ?? "/fiber-config/testnet.yml";
  if (!peerPubkey || !peerWssAddr) {
    throw new Error(
      "live mode needs a Fiber peer: add ?live=1&peer=<pubkey>&wss=<multiaddr/wss> (and optionally &config=<url>)",
    );
  }
  return { configUrl, peerPubkey, peerWssAddr };
}

/** Build the rail for the current URL: live iff ?live=1 (else mock). */
export async function createRail(
  params: URLSearchParams,
  lockCodeHash: string,
  budgetCkb: bigint,
): Promise<DemoRail> {
  await init();
  const wasm = wasmModule as unknown as ControllerWasm;

  if (params.get("live") === "1") {
    return loadController(params, wasm).channel({ mode: "live", peer: peerFromUrl(params) });
  }
  return new MockRail(wasm, lockCodeHash, budgetCkb);
}

/**
 * This browser's controller account address (multi-user mode only). Derived from
 * the persisted per-user keypair — surfaced in the UI so two browsers can be seen
 * to be two distinct on-chain identities. Returns null when ?multi=1 is absent.
 */
export async function userAccountAddress(params: URLSearchParams): Promise<string | null> {
  if (!isMulti(params)) return null;
  await init();
  const wasm = wasmModule as unknown as ControllerWasm;
  return loadController(params, wasm).account.address;
}

// A change cell under the account lock (116-byte session args + 8-byte data)
// needs ~165 CKB of occupied capacity to exist; a live open must leave at least
// this much behind. Using 170 for a small safety margin.
export const ACCOUNT_CHANGE_RESERVE_CKB = 170n;

export interface LiveAccountInfo {
  /** live cells under the account lock — MUST be 1 to open (else MultipleInputs). */
  cellCount: number;
  /** largest single account cell, in CKB (what a channel funds from). */
  cellCapacityCkb: bigint;
  /** max channel budget that still leaves the change reserve behind. */
  maxFundableCkb: bigint;
}

/**
 * Pre-flight the on-chain account before a LIVE open: read its cells so the UI
 * can block a multi-cell account (would fail `MultipleInputs`) and clamp the
 * budget under the change-cell minimum (would fail `CapacityNotEnough`) — the
 * two failures a player hits otherwise. Returns null in mock mode.
 */
export async function liveAccountInfo(params: URLSearchParams): Promise<LiveAccountInfo | null> {
  if (params.get("live") !== "1") return null;
  await init();
  const wasm = wasmModule as unknown as ControllerWasm;
  const controller = loadController(params, wasm);
  const lock = controller.account.lockScript;
  const body = {
    id: 1,
    jsonrpc: "2.0",
    method: "get_cells",
    params: [
      {
        script: { code_hash: lock.code_hash, hash_type: lock.hash_type, args: lock.args },
        script_type: "lock",
        script_search_mode: "exact",
      },
      "asc",
      "0x64",
    ],
  };
  const res = await fetch(controller.rpc, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  }).then((r) => r.json());
  const objs = (res?.result?.objects ?? []) as Array<{ output: { capacity: string } }>;
  const caps = objs.map((o) => BigInt(o.output.capacity));
  const maxShannons = caps.reduce((m, c) => (c > m ? c : m), 0n);
  const cellCapacityCkb = maxShannons / CKB;
  const maxFundableCkb =
    cellCapacityCkb > ACCOUNT_CHANGE_RESERVE_CKB ? cellCapacityCkb - ACCOUNT_CHANGE_RESERVE_CKB : 0n;
  return { cellCount: caps.length, cellCapacityCkb, maxFundableCkb };
}
