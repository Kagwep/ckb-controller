// Demo adapter over @ckb-controller/sdk's channel rails: the UI keeps its
// createRail(params, lockCodeHash, budget) entry point; everything else —
// MockRail, LiveRail, session signing, the funding-tx red-herring handling —
// now lives in the SDK (this demo is its first consumer).
import init, * as wasmModule from "../pkg/controller.js";
import {
  Controller,
  MockRail,
  type ChannelRail,
  type ControllerConfig,
  type ControllerWasm,
  type PeerConfig,
} from "@ckb-controller/sdk";
import { CONFIG, MANIFEST } from "./config.js";

export type DemoRail = ChannelRail;
export { CKB } from "@ckb-controller/sdk";

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
    const controller = Controller.load({
      config: CONFIG as unknown as ControllerConfig,
      manifest: MANIFEST,
      wasm,
    });
    return controller.channel({ mode: "live", peer: peerFromUrl(params) });
  }
  return new MockRail(wasm, lockCodeHash, budgetCkb);
}
