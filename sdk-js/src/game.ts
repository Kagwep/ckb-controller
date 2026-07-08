// The GAME rail (state): session-signed intents posted to an operator that
// batches them into on-chain game-cell transitions. The operator provides
// liveness only — every intent signature and the exact transition are verified
// by the type script on-chain (wire formats §10).
import type { ControllerWasm } from "./types.js";
import { genKey, keyFromHex, signRecoverable, type KeyPair } from "./keys.js";

export interface Board {
  seq: number;
  players: { hash: string; score: number; nonce: number }[];
}

export interface MoveResult {
  seq: number;
  txHash: string;
}

export class GameClient {
  constructor(
    readonly operatorUrl: string,
    readonly gameId: string,
    private wasm: ControllerWasm,
  ) {}

  /** A fresh player identity (one session key per player/tab). */
  player(privHex?: string): GamePlayer {
    return new GamePlayer(this, privHex ? keyFromHex(privHex) : genKey());
  }

  async board(): Promise<Board | null> {
    try {
      const res = await fetch(`${this.operatorUrl}/game`);
      if (!res.ok) return null;
      return (await res.json()) as Board;
    } catch {
      return null;
    }
  }

  async health(): Promise<{ status: string; seq: number; pending: number } | null> {
    try {
      const res = await fetch(`${this.operatorUrl}/health`, { signal: AbortSignal.timeout(3000) });
      return res.ok ? await res.json() : null;
    } catch {
      return null;
    }
  }

  /** @internal */
  encodeIntent(player: KeyPair, points: bigint, nonce: bigint): string {
    const msg = this.wasm.game_intent_message(this.gameId, player.pubHash, points, nonce);
    const sig = signRecoverable(msg, player.priv);
    return this.wasm.game_encode_intent(player.pubHash, points, nonce, sig);
  }

  /** @internal */
  async postIntent(intent: string): Promise<MoveResult> {
    const res = await fetch(`${this.operatorUrl}/intent`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ intent }),
    });
    const body = await res.json();
    if (!res.ok) throw new Error(String(body.error ?? res.status));
    return { seq: body.seq as number, txHash: body.tx_hash as string };
  }
}

/**
 * One player: a session key + the per-player monotonic nonce the type script
 * requires (prev + 1). A rejected move rolls the nonce back so the next try
 * reuses it.
 */
export class GamePlayer {
  private nonce = 0n;

  constructor(
    private client: GameClient,
    readonly key: KeyPair,
  ) {}

  get hash(): string {
    return this.key.pubHash;
  }

  /** Sync the nonce from the current board (idempotent; call after reconnect). */
  syncNonce(board: Board | null): void {
    const me = board?.players.find((p) => p.hash.toLowerCase() === this.hash.toLowerCase());
    if (me) this.nonce = BigInt(me.nonce);
  }

  /** One session-signed move: sign → post → the operator commits on-chain. */
  async move(points: bigint): Promise<MoveResult> {
    this.nonce += 1n;
    try {
      return await this.client.postIntent(this.client.encodeIntent(this.key, points, this.nonce));
    } catch (e) {
      this.nonce -= 1n; // rejected — reuse this nonce next time
      throw e;
    }
  }
}
