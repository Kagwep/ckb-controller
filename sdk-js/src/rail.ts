// The CHANNEL rail (value): one L1 funding tx (session-signed) + N off-chain
// payments + one L1 settle. Two transports behind one interface:
//   MockRail — the wasm in-memory ChannelSession (no node, no funds).
//   LiveRail — a real in-browser Fiber WASM node on public testnet.
// The authorization is identical either way; only the payment transport differs.

export interface OpenResult {
  /** funding tx hash (the on-chain handle; witness-independent). */
  id: string;
  signedBy: string;
}

export interface SettleResult {
  localCkb: bigint;
  remoteCkb: bigint;
  settleTxHash: string;
}

export interface ChannelRail {
  readonly mode: "mock" | "live";
  readonly address: string;
  readonly sessionLabel: string;
  open(budgetCkb: bigint): Promise<OpenResult>;
  /**
   * Resolve only once the channel can route a payment of at least `minCkb`
   * (ChannelReady + outbound liquidity). Mock is instant; live polls the node
   * (~90 s after the funding tx commits). Gate payments on this.
   */
  waitReady(minCkb: bigint): Promise<void>;
  pay(costCkb: bigint): Promise<void>;
  spentCkb(): bigint;
  remainingCkb(): bigint;
  close(): Promise<SettleResult>;
}
