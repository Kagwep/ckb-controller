// MockRail — the wasm in-memory ChannelSession: the full open → pay×N → settle
// loop with real controller addresses, real session signatures, and real
// funding/settle tx SHAPES, just not broadcast. Runs anywhere (no node, no
// funds) — the default demo mode and a fast test harness for rail consumers.
import { utf8ToBytes, bytesToHex } from "@noble/hashes/utils";
import type { ControllerWasm, WasmChannelSession } from "./types.js";
import { CKB } from "./types.js";
import { genKey, signRecoverable, type KeyPair } from "./keys.js";
import type { ChannelRail, OpenResult, PayInvoiceOpts, SettleResult } from "./rail.js";

const HT_DATA2 = 0x04;

// In-page mock invoice registry: mock is single-page, so payer and payee rails
// share this module and one map IS the whole payment network (no hub needed).
interface MockInvoice {
  amountCkb: bigint;
  paid: boolean;
}
const invoices = new Map<string, MockInvoice>();
let invoiceSeq = 0;

export class MockRail implements ChannelRail {
  readonly mode = "mock" as const;
  readonly address: string;
  readonly owner: KeyPair;
  readonly session: KeyPair;
  private ch: WasmChannelSession;

  /**
   * A fresh throwaway controller (new owner + session keys) with a session
   * scoped to a placeholder Fiber funding lock and a spend cap = budget —
   * the same params live mode enforces on-chain.
   */
  constructor(wasm: ControllerWasm, lockCodeHash: string, budgetCkb: bigint) {
    this.owner = genKey();
    this.session = genKey();
    const guardian = "0x" + "00".repeat(20);

    // The single allowed channel destination: a (placeholder) Fiber funding lock.
    const fundingLock = wasm.script("0x" + "ab".repeat(32), 0x00, "0x" + bytesToHex(utf8ToBytes("fiber-funding")));
    const params = wasm.channel_session_params(this.session.pubHash, wasm.no_expiry(), fundingLock, budgetCkb * CKB, guardian);
    const args = wasm.registered_args(this.owner.pubHash, params);

    const accountLock = wasm.script(lockCodeHash, HT_DATA2, args);
    this.address = wasm.controller_address(lockCodeHash, HT_DATA2, args, true);

    // Mock live account cell (1000 CKB) so the channel builders have real shapes.
    const accountInput = "0x" + "11".repeat(32) + ":0";
    const headerDep = "0x" + "00".repeat(32); // no-expiry session needs none
    this.ch = new wasm.ChannelSession(accountLock, accountInput, 1000n * CKB, fundingLock, headerDep);
    this.wasm = wasm;
  }
  private wasm: ControllerWasm;

  get sessionLabel(): string {
    return this.session.pubHash;
  }

  async open(budgetCkb: bigint): Promise<OpenResult> {
    const funding = this.ch.open("game-server-node", budgetCkb * CKB);
    // session-sign the funding tx message (the same signature live mode places
    // in the witness); kept for display/inspection.
    const sig = signRecoverable(funding.message, this.session.priv);
    void this.wasm.session_witness_registered(sig, "", this.wasm.channel_proof_region());
    return { id: funding.outpoint.split(":")[0], signedBy: "session key (mock · no node)" };
  }

  async waitReady(_minCkb: bigint): Promise<void> {
    // in-memory rail routes instantly
  }

  async pay(costCkb: bigint): Promise<void> {
    this.ch.pay(costCkb * CKB);
  }

  async newInvoice(amountCkb: bigint, _description?: string): Promise<string> {
    const id = `mockfibt${amountCkb}n${++invoiceSeq}x${Math.random().toString(36).slice(2, 10)}`;
    invoices.set(id, { amountCkb, paid: false });
    return id;
  }

  async payInvoice(invoice: string, _opts: PayInvoiceOpts = {}): Promise<void> {
    const inv = invoices.get(invoice);
    if (!inv) throw new Error("unknown invoice — mock invoices only route within this page");
    if (inv.paid) throw new Error("invoice already paid");
    this.ch.pay(inv.amountCkb * CKB); // same budget guard/accounting as pay()
    inv.paid = true;
  }

  async waitInvoicePaid(invoice: string, timeoutMs = 30000): Promise<void> {
    const inv = invoices.get(invoice);
    if (!inv) throw new Error("unknown invoice — mock invoices only route within this page");
    const deadline = Date.now() + timeoutMs;
    while (!inv.paid) {
      if (Date.now() > deadline) throw new Error(`invoice not paid within ${timeoutMs / 1000}s (mock)`);
      await new Promise((r) => setTimeout(r, 50));
    }
  }

  spentCkb(): bigint {
    return BigInt(this.ch.spent()) / CKB;
  }
  remainingCkb(): bigint {
    return BigInt(this.ch.remaining()) / CKB;
  }

  async close(): Promise<SettleResult> {
    const res = this.ch.close();
    // the settle tx message is session-signable too
    signRecoverable(res.settle.message, this.session.priv);
    return {
      localCkb: BigInt(res.local) / CKB,
      remoteCkb: BigInt(res.remote) / CKB,
      settleTxHash: res.settle.outpoint.split(":")[0],
    };
  }
}
