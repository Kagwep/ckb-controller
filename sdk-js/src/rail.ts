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

export interface PayInvoiceOpts {
  /**
   * Trampoline node pubkeys (Phase 2: `[hubPubkey]` — the channel peer). The
   * payer needs no network graph; the hub pathfinds to the invoice payee.
   */
  trampolineHops?: string[];
  /** Max routing fee in whole CKB. Be generous — fee estimation with gossip off is rough. */
  maxFeeCkb?: bigint;
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
  /** Receive side: issue an invoice for `amountCkb`; returns the pasteable invoice string. */
  newInvoice(amountCkb: bigint, description?: string): Promise<string>;
  /**
   * Pay a peer's invoice — counts against the budget like pay(). Live resolves
   * only once the payment reaches Success (rejects on Failed); route through the
   * hub with opts.trampolineHops. Mock settles against the in-page registry.
   */
  payInvoice(invoice: string, opts?: PayInvoiceOpts): Promise<void>;
  /**
   * Receive side: resolve once an invoice THIS rail issued is Paid (poll);
   * rejects on Cancelled/Expired or timeout.
   */
  waitInvoicePaid(invoice: string, timeoutMs?: number): Promise<void>;
  spentCkb(): bigint;
  remainingCkb(): bigint;
  close(): Promise<SettleResult>;
}
