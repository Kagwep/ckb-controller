// account — single-cell hygiene for the controller account (the lock allows ONE
// account input per tx, so the account must stay a single live cell):
//
//   account show           list the account's live cells
//   account grow <ckb>     top up IN PLACE (session witness + one sighash input
//                          -> one bigger account cell; inflow, cap untouched)
//   account drain          session-sign the SMALLEST account cell back to the
//                          key's sighash lock (run after a settle leaves 2 cells)
//
// Dry-run by default; --send broadcasts. Ports of demo/grow-account.mjs and
// demo/drain-account.mjs on the shared config lib.
import { ccc } from "@ckb-ccc/core";
import { secp256k1 } from "@noble/curves/secp256k1";
import { loadCtx, makeSigner, accountLock, fmtCkb } from "../lib/config.mjs";

const FEE = 100000n; // 0.001 CKB

async function listCells(client, lock) {
  const cells = [];
  for await (const cell of client.findCellsByLock(lock, null, true)) cells.push(cell);
  return cells;
}

function sessionWitness(wasm, sessionPriv, tx) {
  const msg = wasm.tx_message(ccc.hexFrom(tx.toBytes()));
  const s = secp256k1.sign(msg.slice(2), sessionPriv);
  const sig = "0x" + Buffer.from([...s.toCompactRawBytes(), s.recovery]).toString("hex");
  return wasm.session_witness_registered(sig, "", wasm.channel_proof_region());
}

export async function run(args, { configPath }) {
  const sub = args[0];
  const send = args.includes("send") || args.includes("--send");
  const ctx = await loadCtx(configPath);
  const { client, signer } = await makeSigner(ctx);
  const { lock, address, sessionPriv, wasm } = await accountLock(ctx);

  const cells = await listCells(client, lock);

  if (!sub || sub === "show") {
    console.log(`account: ${address}`);
    cells.forEach((c, i) =>
      console.log(`  cell ${i + 1}: ${c.outPoint.txHash}:${BigInt(c.outPoint.index)} = ${fmtCkb(c.cellOutput.capacity)}`),
    );
    console.log(
      cells.length === 1 ? "OK: single account cell" : cells.length === 0 ? "no account cell — `ckb-controller deploy`" : `PROBLEM: ${cells.length} cells (must be 1) — \`account drain --send\``,
    );
    return;
  }

  const lockDep = ctx.net.lock?.dep;
  const authDep = ctx.net.auth?.dep;
  if (!lockDep || !authDep) throw new Error("manifest missing lock/auth deps — `ckb-controller deploy` first");

  if (sub === "grow") {
    const growCkb = args[1] && !isNaN(Number(args[1])) ? args[1] : "700";
    const GROW = ccc.fixedPointFrom(growCkb);
    if (cells.length !== 1) throw new Error(`account must be a single cell (found ${cells.length}) — drain first`);
    const account = cells[0];
    const accCap = account.cellOutput.capacity;

    const sighashLock = (await signer.getRecommendedAddressObj()).script;
    const need = GROW + ccc.fixedPointFrom("62") + FEE;
    let feeCell = null;
    for await (const cell of client.findCellsByLock(sighashLock, null, true)) {
      if (cell.outputData === "0x" && !cell.cellOutput.type && cell.cellOutput.capacity >= need) {
        feeCell = cell;
        break;
      }
    }
    if (!feeCell) throw new Error(`no plain sighash cell with ${fmtCkb(need)}`);

    const tx = ccc.Transaction.from({
      inputs: [
        { previousOutput: account.outPoint, since: 0n },
        { previousOutput: feeCell.outPoint, since: 0n },
      ],
      cellDeps: [
        { outPoint: lockDep, depType: "code" },
        { outPoint: authDep, depType: "code" },
      ],
      outputs: [
        { lock, capacity: accCap + GROW },
        { lock: sighashLock, capacity: feeCell.cellOutput.capacity - GROW - FEE },
      ],
      outputsData: ["0x", "0x"],
      witnesses: ["0x", "0x"],
    });
    tx.witnesses[0] = sessionWitness(wasm, sessionPriv, tx);
    console.log(`grow: account ${fmtCkb(accCap)} -> ${fmtCkb(accCap + GROW)} (sighash input ${feeCell.outPoint.txHash.slice(0, 10)}…)`);
    if (send) {
      const hash = await signer.sendTransaction(tx); // signs its sighash group only
      console.log(`BROADCAST: ${hash}\nexplorer: ${ctx.explorer(hash)}`);
    } else {
      console.log("DRY RUN — add --send to broadcast.");
    }
    return;
  }

  if (sub === "drain") {
    if (cells.length === 0) throw new Error("no account cell to drain");
    const small = cells.reduce((a, b) => (a.cellOutput.capacity <= b.cellOutput.capacity ? a : b));
    const cap = small.cellOutput.capacity;
    const capCkb = cap / 100000000n;
    if (capCkb > BigInt(ctx.config.session.spendCapCkb)) {
      throw new Error(
        `smallest cell holds ${capCkb} CKB > spend cap ${ctx.config.session.spendCapCkb} CKB — a session drain would be rejected on-chain (SpendCapExceeded); use the owner key`,
      );
    }
    const destLock = (await signer.getRecommendedAddressObj()).script;
    const tx = ccc.Transaction.from({
      inputs: [{ previousOutput: small.outPoint, since: 0n }],
      cellDeps: [
        { outPoint: lockDep, depType: "code" },
        { outPoint: authDep, depType: "code" },
      ],
      outputs: [{ lock: destLock, capacity: cap - FEE }],
      outputsData: ["0x"],
      witnesses: ["0x"],
    });
    tx.witnesses = [sessionWitness(wasm, sessionPriv, tx)];
    console.log(`drain: ${small.outPoint.txHash.slice(0, 10)}…:${BigInt(small.outPoint.index)} (${fmtCkb(cap)}) -> sighash key`);
    if (send) {
      const hash = await client.sendTransaction(tx);
      console.log(`BROADCAST: ${hash}\nexplorer: ${ctx.explorer(hash)}`);
    } else {
      console.log("DRY RUN — add --send to broadcast.");
    }
    return;
  }

  throw new Error(`unknown subcommand "${sub}" (show | grow <ckb> | drain)`);
}
