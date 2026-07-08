// LiveRail — a real Fiber node running IN THE BROWSER (@nervosnetwork/fiber-js,
// fiber-wasm) on public testnet. The channel is funded via
// `openChannelWithExternalFunding`: Fiber builds the funding tx, and the
// controller SESSION key signs it witness-only, bounded by the on-chain spend
// cap. The user's wallet key is never given to the Fiber node — this is the
// authorization layer Fiber's WASM docs recommend but don't provide.
//
// Hard-won operational knowledge preserved from the first live run (2026-07-08):
//  - connectPeer returns before the WSS handshake completes → poll listPeers.
//  - the channel is routable only after ChannelReady + outbound liquidity
//    (~90 s after the funding tx commits) → waitReady polls listChannels.
//  - the browser's own funding-tx broadcast error (`Inputs[1].Lock` -11) is a
//    RED HERRING: the acceptor fnn broadcasts the fully-signed same-hash tx.
//    Nothing here treats that console error as fatal.
//  - needs crossOriginIsolated (COOP/COEP) for SharedArrayBuffer.
import type { Account } from "./account.js";
import type { Session } from "./session.js";
import { fundingTxHash } from "./session.js";
import type { ChannelRail, OpenResult, SettleResult } from "./rail.js";
import { CKB } from "./types.js";

export interface PeerConfig {
  /** URL of the Fiber network config (e.g. /fiber-config/testnet.yml). */
  configUrl: string;
  peerPubkey: string;
  /** Dial multiaddr — MUST include /p2p/<peerId> (e.g. /dns4/…/tcp/443/wss/p2p/…). */
  peerWssAddr: string;
}

const normPk = (s: string) => s.replace(/^0x/i, "").toLowerCase();

/* eslint-disable @typescript-eslint/no-explicit-any */
async function waitForPeer(fiber: any, pubkey: string, timeoutMs: number): Promise<void> {
  const target = normPk(pubkey);
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const { peers } = await fiber.listPeers();
      if (peers?.some((p: { pubkey: string }) => normPk(p.pubkey) === target)) return;
    } catch {
      /* keep polling */
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error(
    `peer ${pubkey.slice(0, 14)}… did not connect within ${timeoutMs / 1000}s — check the WSS tunnel + /p2p/<peerId> address`,
  );
}

export class LiveRail implements ChannelRail {
  readonly mode = "live" as const;
  private budget = 0n;
  private spent = 0n;
  private channelId = "";

  private constructor(
    readonly address: string,
    readonly sessionLabel: string,
    private fiber: any,
    private peer: PeerConfig,
    private account: Account,
    private session: Session,
  ) {}

  /** Boot the in-browser Fiber node and connect it to the peer. */
  static async create(account: Account, session: Session, peer: PeerConfig): Promise<LiveRail> {
    if (typeof self !== "undefined" && !(self as any).crossOriginIsolated) {
      throw new Error("Not crossOriginIsolated — the Fiber WASM node needs COOP/COEP headers (set in vite.config.ts).");
    }
    // Literal dynamic import: bundlers code-split fiber-js into a lazy chunk;
    // mock-only consumers never load it.
    const fiberMod: any = await import("@nervosnetwork/fiber-js");
    const { Fiber, randomSecretKey } = fiberMod;

    const config = await fetch(peer.configUrl).then((r) => {
      if (!r.ok) throw new Error(`failed to load fiber config: ${peer.configUrl}`);
      return r.text();
    });

    const fiber = new Fiber();
    // node identity key + an internal ckb key — NEVER the wallet key; funds
    // arrive via external funding signed by the controller session.
    await fiber.start(config, randomSecretKey(), randomSecretKey(), undefined, "info", "testnet:controller-sdk");
    await fiber.connectPeer({ pubkey: peer.peerPubkey, address: peer.peerWssAddr, addr_type: "wss", save: true });
    await waitForPeer(fiber, peer.peerPubkey, 30000);

    return new LiveRail(account.address, session.pubHash, fiber, peer, account, session);
  }

  async open(budgetCkb: bigint): Promise<OpenResult> {
    this.budget = budgetCkb;
    this.spent = 0n;
    const budgetShannons = budgetCkb * CKB;

    // Fiber builds the funding tx; the controller account is the external wallet.
    const result = await this.fiber.openChannelWithExternalFunding({
      pubkey: this.peer.peerPubkey,
      funding_amount: "0x" + budgetShannons.toString(16),
      public: false,
      shutdown_script: this.account.lockScript, // settled balance returns to the account
      funding_lock_script: this.account.lockScript,
      funding_lock_script_cell_deps: this.account.lockCellDeps,
      funding_fee_rate: "0x7d0",
    });

    // The SESSION key signs Fiber's unsigned funding tx (witness only).
    const signed = this.session.signFundingTx(result.unsigned_funding_tx);
    await this.fiber.submitSignedFundingTx({ channel_id: result.channel_id, signed_funding_tx: signed });

    this.channelId = result.channel_id as string;
    // tx hash is witness-independent — compute it from the unsigned tx.
    return { id: fundingTxHash(result.unsigned_funding_tx), signedBy: "session key (live · in-browser Fiber node)" };
  }

  async waitReady(minCkb: bigint, timeoutMs = 180000): Promise<void> {
    const minShannons = minCkb * CKB;
    const deadline = Date.now() + timeoutMs;
    let last = "";
    while (Date.now() < deadline) {
      try {
        const res = await this.fiber.listChannels({ include_closed: false });
        const ch = res?.channels?.find((c: any) => c.channel_id === this.channelId);
        if (ch) {
          last = `${ch.state?.state_name} local=${BigInt(ch.local_balance ?? "0x0")}`;
          if (ch.state?.state_name === "ChannelReady" && BigInt(ch.local_balance ?? "0x0") >= minShannons) return;
        }
      } catch {
        /* node may not list the channel yet */
      }
      await new Promise((r) => setTimeout(r, 2000));
    }
    throw new Error(`channel ${this.channelId.slice(0, 12)}… not routable within ${timeoutMs / 1000}s (last: ${last || "not listed"})`);
  }

  async pay(costCkb: bigint): Promise<void> {
    // keysend micropayment to the peer — off-chain, no L1, no popup.
    await this.fiber.sendPayment({
      target_pubkey: this.peer.peerPubkey,
      amount: "0x" + (costCkb * CKB).toString(16),
      keysend: true,
    });
    this.spent += costCkb;
  }

  spentCkb(): bigint {
    return this.spent;
  }
  remainingCkb(): bigint {
    return this.budget - this.spent;
  }

  async close(): Promise<SettleResult> {
    await this.fiber.shutdownChannel({ channel_id: this.channelId, force: false, fee_rate: "0x7d0" });
    let settleTxHash = "";
    // best-effort: surface the cooperative-close tx hash for the explorer link.
    for (let i = 0; i < 10; i++) {
      try {
        const res = await this.fiber.listChannels({ include_closed: true });
        const ch = res?.channels?.find((c: { channel_id: string }) => c.channel_id === this.channelId);
        if (ch?.shutdown_transaction_hash) {
          settleTxHash = ch.shutdown_transaction_hash;
          break;
        }
      } catch {
        /* node may not have the close tx yet */
      }
      await new Promise((r) => setTimeout(r, 1000));
    }
    return { localCkb: this.budget - this.spent, remoteCkb: this.spent, settleTxHash };
  }

  stop(): Promise<void> {
    return this.fiber.stop();
  }
}
