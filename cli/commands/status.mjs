// status — one screen of truth: chain, code cells, account, game cell, operator.
import { ccc } from "@ckb-ccc/core";
import { loadCtx, accountLock, fmtCkb } from "../lib/config.mjs";

const ok = (s) => `  ✓ ${s}`;
const bad = (s) => `  ✗ ${s}`;
const warn = (s) => `  ! ${s}`;

export async function run(_args, { configPath }) {
  const ctx = await loadCtx(configPath);
  console.log(`config:  ${ctx.configPath}`);
  console.log(`network: ${ctx.network}   rpc: ${ctx.rpc}`);

  const client = new ccc.ClientPublicTestnet({ url: ctx.rpc });

  // chain
  try {
    const tip = await client.getTip();
    console.log(ok(`chain reachable, tip #${tip}`));
  } catch (e) {
    console.log(bad(`chain unreachable: ${e.message}`));
    return;
  }

  // code cells
  for (const name of ["lock", "auth", "secp256k1Sighash", "game"]) {
    const art = ctx.net[name];
    if (!art?.dep) {
      console.log(warn(`${name}: not in manifest (run \`ckb-controller deploy\`)`));
      continue;
    }
    try {
      const cell = await client.getCellLive({ txHash: art.dep.txHash, index: art.dep.index }, false);
      console.log(
        cell
          ? ok(`${name}: live code cell ${art.dep.txHash.slice(0, 10)}…:${BigInt(art.dep.index)}`)
          : bad(`${name}: dep ${art.dep.txHash.slice(0, 10)}… NOT live (consumed?) — redeploy`),
      );
    } catch (e) {
      console.log(bad(`${name}: ${e.message}`));
    }
  }

  // account (derived from config keys)
  let account;
  try {
    account = await accountLock(ctx);
  } catch (e) {
    console.log(warn(`account: ${e.message}`));
  }
  if (account) {
    let cells = 0,
      total = 0n;
    for await (const cell of client.findCellsByLock(account.lock, null, true)) {
      cells++;
      total += cell.cellOutput.capacity;
    }
    const line = `account: ${cells} cell(s), ${fmtCkb(total)}  (${account.address.slice(0, 24)}…)`;
    if (cells === 1) console.log(ok(line));
    else if (cells === 0) console.log(warn(`${line} — create it: \`ckb-controller deploy\``));
    else console.log(warn(`${line} — must be 1: \`ckb-controller account drain --send\``));

    // game cell (needs the wasm already loaded by accountLock)
    const game = ctx.net.game;
    if (game?.codeHash) {
      const typeScript = ccc.Script.from({ codeHash: game.codeHash, hashType: "data2", args: ctx.gameId });
      let cell = null;
      for await (const c of client.findCellsByType(typeScript, true)) {
        cell = c;
        break;
      }
      if (cell) {
        const board = JSON.parse(account.wasm.game_decode_state(cell.outputData));
        console.log(ok(`game cell: seq ${board.seq}, ${board.players.length} player(s), ${fmtCkb(cell.cellOutput.capacity)}`));
      } else {
        console.log(warn(`game cell: none for game ${ctx.gameId.slice(0, 10)}… — \`ckb-controller deploy\` creates it`));
      }
    }
  }

  // operator
  const listen = ctx.config.operator?.listen ?? "127.0.0.1:9944";
  try {
    const res = await fetch(`http://${listen}/health`, { signal: AbortSignal.timeout(3000) });
    const h = await res.json();
    console.log(ok(`operator on ${listen}: ${h.status}, seq ${h.seq}, ${h.pending} pending`));
  } catch {
    console.log(warn(`operator not running on ${listen} — \`cargo run -p paymaster-service --bin game-operator\``));
  }
}
