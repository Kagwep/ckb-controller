// The SESSION: the key the owner blessed, and everything it may sign —
// tx messages (blake2b of the raw tx with cell_deps cleared) and, for the
// channel rail, Fiber's unsigned funding tx (WITNESS-ONLY: the input/output
// structure Fiber froze is never touched; witnesses aren't part of
// RawTransaction, so splicing the witness never changes the signed message).
import { ccc } from "@ckb-ccc/core";
import type { ControllerWasm } from "./types.js";
import { signRecoverable, type KeyPair } from "./keys.js";
import type { CkbJsonRpcTransaction } from "@nervosnetwork/fiber-js";

/** Map a CKB JSON-RPC tx (snake_case hex) to a CCC TransactionLike. */
function toCccTxLike(json: CkbJsonRpcTransaction) {
  return {
    version: json.version,
    cellDeps: json.cell_deps.map((d) => ({
      outPoint: { txHash: d.out_point.tx_hash, index: d.out_point.index },
      depType: d.dep_type === "dep_group" ? ("depGroup" as const) : d.dep_type,
    })),
    headerDeps: json.header_deps,
    inputs: json.inputs.map((i) => ({
      previousOutput: { txHash: i.previous_output.tx_hash, index: i.previous_output.index },
      since: i.since,
    })),
    outputs: json.outputs.map((o) => ({
      capacity: o.capacity,
      lock: { codeHash: o.lock.code_hash, hashType: o.lock.hash_type, args: o.lock.args },
      type: o.type ? { codeHash: o.type.code_hash, hashType: o.type.hash_type, args: o.type.args } : undefined,
    })),
    outputsData: json.outputs_data,
    witnesses: json.witnesses,
  };
}

/** Molecule-encode a fiber JSON tx (for the controller signing message). */
export function fiberTxToMoleculeHex(json: CkbJsonRpcTransaction): string {
  return ccc.hexFrom(ccc.Transaction.from(toCccTxLike(json)).toBytes());
}

/**
 * The CKB tx hash of a funding tx. Witnesses aren't part of RawTransaction, so
 * the unsigned tx's hash equals the committed (signed) funding tx hash —
 * usable for an explorer link before/after signing.
 */
export function fundingTxHash(json: CkbJsonRpcTransaction): string {
  return ccc.Transaction.from(toCccTxLike(json)).hash();
}

/** The session key + its signing operations. */
export class Session {
  constructor(
    readonly key: KeyPair,
    private wasm: ControllerWasm,
  ) {}

  get pubHash(): string {
    return this.key.pubHash;
  }

  /** Sign a 32-byte 0x-hex message (recoverable, 65 bytes). */
  sign(msgHex: string): string {
    return signRecoverable(msgHex, this.key.priv);
  }

  /** The registered-model SESSION witness for a wildcard/no-proof tx. */
  witness(sessionSig: string): string {
    return this.wasm.session_witness_registered(sessionSig, "", this.wasm.channel_proof_region());
  }

  /** The controller signing message for a Fiber funding tx. */
  fundingTxMessage(unsignedTx: CkbJsonRpcTransaction): string {
    return this.wasm.tx_message(fiberTxToMoleculeHex(unsignedTx));
  }

  /**
   * Witness-only funding-tx signer (fiber-js external-funding contract).
   * `accountInputIndex` = the account input's witness slot (default 0 — the
   * external funder leads the inputs in Fiber's funding tx).
   */
  signFundingTx(unsignedTx: CkbJsonRpcTransaction, accountInputIndex = 0): CkbJsonRpcTransaction {
    const witness = this.witness(this.sign(this.fundingTxMessage(unsignedTx)));
    const witnesses = [...unsignedTx.witnesses] as string[];
    while (witnesses.length <= accountInputIndex) witnesses.push("0x");
    witnesses[accountInputIndex] = witness;
    return { ...unsignedTx, witnesses: witnesses as CkbJsonRpcTransaction["witnesses"] };
  }
}
