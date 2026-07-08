// game — the shared game cell's hygiene:
//
//   game show          decode + print the live board (seq, players, capacity)
//   game grow <ckb>    enlarge the game cell IN PLACE. Each player entry is 36
//                      bytes = 36 CKB of occupied capacity, and every transition
//                      shrinks the cell a hair (self-funded fee) — a full cell
//                      rejects new players with InsufficientCellCapacity. The
//                      type script allows extra non-group inputs and an EMPTY
//                      batch is a no-op transition, so: [game cell, sighash cell]
//                      -> [bigger game cell (same data), change], witness =
//                      empty batch + the operator key's sighash signature.
//
// Dry-run by default; --send broadcasts.
import { ccc } from "@ckb-ccc/core";
import { loadCtx, makeSigner, initWasm, fmtCkb } from "../lib/config.mjs";

const FEE = 100000n; // 0.001 CKB

export async function run(args, { configPath }) {
  const sub = args[0] ?? "show";
  const send = args.includes("send") || args.includes("--send");
  const ctx = await loadCtx(configPath);
  const { client, signer } = await makeSigner(ctx);
  const wasm = await initWasm(ctx);

  const game = ctx.net.game;
  if (!game?.codeHash) throw new Error(`manifest has no game script for "${ctx.network}" — deploy first`);
  const typeScript = ccc.Script.from({ codeHash: game.codeHash, hashType: "data2", args: ctx.gameId });

  let cell = null;
  for await (const c of client.findCellsByType(typeScript, true)) {
    cell = c;
    break;
  }
  if (!cell) throw new Error(`no live game cell for game ${ctx.gameId.slice(0, 10)}… — \`ckb-controller deploy\``);

  const board = JSON.parse(wasm.game_decode_state(cell.outputData));
  const capacity = cell.cellOutput.capacity;
  // occupied = cell overhead (8 cap + 32+1+len(lock args) + 32+1+32 type) + data
  const occupied = ccc.fixedPointFrom(String(cell.cellOutput.occupiedSize)) + BigInt(cell.outputData.length / 2 - 1) * 100000000n;

  if (sub === "show") {
    console.log(`game cell: ${cell.outPoint.txHash}:${BigInt(cell.outPoint.index)}`);
    console.log(`  seq ${board.seq}, ${board.players.length} player(s)`);
    console.log(`  capacity ${fmtCkb(capacity)}, headroom ≈ ${fmtCkb(capacity - occupied)} (${(capacity - occupied) / (36n * 100000000n)} more players)`);
    for (const p of board.players) console.log(`  ${p.hash} score ${p.score} nonce ${p.nonce}`);
    return;
  }

  if (sub === "grow") {
    const growCkb = args[1] && !isNaN(Number(args[1])) ? args[1] : "500";
    const GROW = ccc.fixedPointFrom(growCkb);

    const keyLock = (await signer.getRecommendedAddressObj()).script;
    const need = GROW + ccc.fixedPointFrom("62") + FEE;
    let feeCell = null;
    for await (const c of client.findCellsByLock(keyLock, null, true)) {
      if (c.outputData === "0x" && !c.cellOutput.type && c.cellOutput.capacity >= need) {
        feeCell = c;
        break;
      }
    }
    if (!feeCell) throw new Error(`no plain sighash cell with ${fmtCkb(need)}`);

    const tx = ccc.Transaction.from({
      inputs: [
        { previousOutput: cell.outPoint, since: 0n },
        { previousOutput: feeCell.outPoint, since: 0n },
      ],
      cellDeps: [
        { outPoint: game.dep, depType: "code" },
        { outPoint: ctx.net.auth.dep, depType: "code" },
      ],
      outputs: [
        { lock: cell.cellOutput.lock, type: cell.cellOutput.type, capacity: capacity + GROW },
        { lock: keyLock, capacity: feeCell.cellOutput.capacity - GROW - FEE },
      ],
      outputsData: [cell.outputData, "0x"], // state unchanged = no-op transition
      witnesses: ["0x", "0x"],
    });
    // empty intent batch (n=0): the type script verifies a no-op transition.
    tx.setWitnessArgsAt(0, ccc.WitnessArgs.from({ inputType: "0x0000" }));

    console.log(`grow game cell: ${fmtCkb(capacity)} -> ${fmtCkb(capacity + GROW)} (+${(GROW) / (36n * 100000000n)} player slots)`);
    if (send) {
      const hash = await signer.sendTransaction(tx); // signs both sighash inputs
      console.log(`BROADCAST: ${hash}\nexplorer: ${ctx.explorer(hash)}`);
    } else {
      console.log("DRY RUN — add --send to broadcast.");
    }
    return;
  }

  throw new Error(`unknown subcommand "${sub}" (show | grow <ckb>)`);
}
