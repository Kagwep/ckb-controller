// @ckb-controller/sdk — the runtime game devs code against.
//
//   const controller = Controller.load({ config, manifest, wasm });
//   const game = controller.game();               // state rail (operator + type script)
//   const player = game.player();
//   await player.move(5n);                         // session-signed, no popup, on-chain verified
//
//   const rail = await controller.channel({ mode: "live", peer });  // value rail (Fiber)
//   await rail.open(500n);                         // ONE session-signed L1 funding tx
//   await rail.waitReady(5n);
//   await rail.pay(5n);                            // off-chain, instant, no L1
//   await rail.close();                            // cooperative settle back to the account
//
// The SDK is pure logic: the caller supplies the parsed config pair and the
// INITIALISED controller-wasm module (see types.ts) — no file or wasm loading
// here, so it runs identically in the browser (Vite) and Node.
import type { ControllerConfig, ControllerWasm, NetworkManifest } from "./types.js";
import { deriveAccount, type Account, type UserKeys } from "./account.js";
import { Session } from "./session.js";
import { GameClient } from "./game.js";
import { MockRail } from "./mock.js";
import { LiveRail, type PeerConfig } from "./live.js";
import type { ChannelRail } from "./rail.js";

export * from "./types.js";
export * from "./keys.js";
export { deriveAccount, type Account, type UserKeys } from "./account.js";
export { Session, fundingTxHash, fiberTxToMoleculeHex } from "./session.js";
export {
  GameClient,
  GamePlayer,
  type Board,
  type MoveResult,
  type RelayInvoice,
  type ResultEvent,
} from "./game.js";
export { MockRail } from "./mock.js";
export { LiveRail, type PeerConfig } from "./live.js";
export type { ChannelRail, OpenResult, PayInvoiceOpts, SettleResult } from "./rail.js";

export interface LoadOptions {
  config: ControllerConfig;
  manifest: Record<string, NetworkManifest>;
  wasm: ControllerWasm;
  /** Override config.network (e.g. "devnet"). */
  network?: string;
  /** Per-user keypair override — replaces the config.session privkeys (see UserKeys). */
  keys?: UserKeys;
}

export type ChannelOptions = { mode: "mock"; budgetCkb: bigint } | { mode: "live"; peer: PeerConfig };

export class Controller {
  readonly network: string;
  readonly net: NetworkManifest;
  readonly account: Account;
  readonly session: Session;

  private constructor(
    readonly config: ControllerConfig,
    net: NetworkManifest,
    network: string,
    private wasm: ControllerWasm,
    keys?: UserKeys,
  ) {
    this.network = network;
    this.net = net;
    this.account = deriveAccount(config, net, wasm, keys);
    this.session = new Session(this.account.session, wasm);
  }

  static load(opts: LoadOptions): Controller {
    const network = opts.network ?? opts.config.network;
    const net = opts.manifest[network];
    if (!net) throw new Error(`manifest has no network "${network}"`);
    return new Controller(opts.config, net, network, opts.wasm, opts.keys);
  }

  get rpc(): string {
    return this.config.networks[this.network].rpc;
  }

  explorerTx(hash: string): string {
    const base = this.config.networks[this.network].explorerTx ?? "";
    return base ? `${base}${hash}` : hash;
  }

  /** The state rail: session-signed intents via the operator. */
  game(operatorUrl?: string, gameId?: string): GameClient {
    const url = (operatorUrl ?? `http://${this.config.operator?.listen ?? "127.0.0.1:9944"}`).replace(/\/$/, "");
    return new GameClient(url, gameId ?? this.config.gameId, this.wasm);
  }

  /** The value rail: a budget-capped payment channel (mock or live Fiber). */
  async channel(opts: ChannelOptions): Promise<ChannelRail> {
    if (opts.mode === "mock") {
      const lockCodeHash = this.net.lock?.codeHash ?? "0x" + "cd".repeat(32);
      return new MockRail(this.wasm, lockCodeHash, opts.budgetCkb);
    }
    return LiveRail.create(this.account, this.session, opts.peer);
  }
}
