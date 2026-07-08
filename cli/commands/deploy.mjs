// deploy — make the configured network fully game-ready, deploying ONLY what's
// missing, in dependency order:
//
//   1. code cells: lock (build/release/controller-session-lock), ckb-auth
//      (deps/auth), game (build/release/controller-game-cell) — data cells whose
//      data hash is the script's code_hash; manifest is updated per deploy.
//   2. the game cell (genesis, empty state, type = game script) for config gameId.
//   3. the controller account cell (capacity from --account-ckb, default 500).
//
// Dry-run by default; --send broadcasts (and waits for each stage a later stage
// depends on). Inputs are restricted to PLAIN cells (no data, no type), so the
// key's live code cells can never be spent by accident.
//
// The core (`ensureDeployed`) takes an injected client + signer so the `dev`
// command can reuse it against a local devnet with a custom script registry.
import { ccc } from "@ckb-ccc/core";
import { readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { loadCtx, saveArtifact, makeSigner, accountLock, selectPlainCells, assertOnlyInputs, fmtCkb, CKB } from "../lib/config.mjs";

const CODE_CELLS = [
  { name: "lock", rel: "build/release/controller-session-lock", what: "controller session lock" },
  { name: "auth", rel: "deps/auth", what: "ckb-auth (signature verifier)" },
  { name: "game", rel: "build/release/controller-game-cell", what: "game aggregator type script" },
];

async function waitLive(client, outPoint, what, onTick, tries = 60) {
  for (let i = 0; i < tries; i++) {
    if (await client.getCellLive(outPoint, false)) return;
    await onTick?.();
    await new Promise((r) => setTimeout(r, 2000));
  }
  throw new Error(`timeout waiting for ${what} (${outPoint.txHash}) to commit`);
}

/**
 * Ensure code cells + game cell + account exist on `client`'s chain, updating
 * ctx's manifest as artifacts land. opts: { send, accountCkb, gameCellCkb,
 * onTick (called while waiting — the dev command mines blocks here) }.
 */
export async function ensureDeployed(ctx, client, signer, opts = {}) {
  const { send = false, accountCkb = 500n, gameCellCkb = 500n, onTick } = opts;
  const keyLock = (await signer.getRecommendedAddressObj()).script;

  // ── 1. code cells ──────────────────────────────────────────────────────────
  for (const cc of CODE_CELLS) {
    const art = ctx.net[cc.name];
    const binPath = join(ctx.root, cc.rel);
    // A locally built binary is the source of truth: if its data hash differs
    // from the manifest (e.g. the game rules changed), the live old deployment
    // does NOT satisfy this artifact — deploy the new one.
    const localHash = existsSync(binPath) ? ccc.hashCkb(new Uint8Array(await readFile(binPath))) : null;
    const manifestCurrent = !art?.codeHash || !localHash || art.codeHash === localHash;
    if (art?.dep && manifestCurrent && (await client.getCellLive({ txHash: art.dep.txHash, index: art.dep.index }, false))) {
      console.log(`  = ${cc.name}: already live (${art.dep.txHash.slice(0, 10)}…) — skipped`);
      continue;
    }
    if (art?.dep && !manifestCurrent) {
      console.log(`  ~ ${cc.name}: local binary differs from the deployed code (rules changed?) — deploying the new version`);
    }
    if (!existsSync(binPath)) {
      throw new Error(`${cc.name}: binary missing at ${cc.rel} — build first (\`ckb-controller build\`)`);
    }
    const code = new Uint8Array(await readFile(binPath));
    const codeHash = ccc.hashCkb(code);
    const capacity = ccc.fixedPointFrom(String(code.length + 200));
    console.log(`  + ${cc.name}: deploy ${cc.what}, ${code.length} bytes ≈ ${fmtCkb(capacity)} (code_hash ${codeHash.slice(0, 10)}…)`);
    if (!send) continue;

    const { inputs } = await selectPlainCells(client, keyLock, capacity + ccc.fixedPointFrom("500"));
    const tx = ccc.Transaction.from({ outputs: [{ lock: keyLock, capacity }], outputsData: [ccc.hexFrom(code)] });
    for (const c of inputs) tx.inputs.push(ccc.CellInput.from({ previousOutput: c.outPoint, since: 0n }));
    await tx.completeFeeBy(signer, 1000n);
    assertOnlyInputs(tx, inputs.map((c) => c.outPoint));
    const hash = await signer.sendTransaction(tx);
    console.log(`    broadcast ${hash} — waiting for commit…`);
    await waitLive(client, { txHash: hash, index: "0x0" }, cc.name, onTick);
    await saveArtifact(ctx, cc.name, {
      codeHash,
      hashType: "data2",
      dep: { txHash: hash, index: "0x0" },
      depType: "code",
    });
    console.log(`    ✓ ${cc.name} live, manifest updated`);
  }

  // ── 2. the game cell (genesis) ─────────────────────────────────────────────
  const game = ctx.net.game;
  if (game?.codeHash) {
    const typeScript = ccc.Script.from({ codeHash: game.codeHash, hashType: "data2", args: ctx.gameId });
    let gameCell = null;
    for await (const c of client.findCellsByType(typeScript, false)) {
      gameCell = c;
      break;
    }
    if (gameCell) {
      console.log(`  = game cell: live for game ${ctx.gameId.slice(0, 10)}… — skipped`);
    } else {
      console.log(`  + game cell: genesis ${gameCellCkb} CKB, empty state, game ${ctx.gameId.slice(0, 10)}…`);
      if (send) {
        const capacity = gameCellCkb * CKB;
        const { inputs } = await selectPlainCells(client, keyLock, capacity + ccc.fixedPointFrom("200"));
        const tx = ccc.Transaction.from({
          outputs: [{ lock: keyLock, type: typeScript, capacity }],
          outputsData: ["0x"], // empty state == genesis
        });
        for (const c of inputs) tx.inputs.push(ccc.CellInput.from({ previousOutput: c.outPoint, since: 0n }));
        // the type script executes on genesis -> its code cell must be a dep
        tx.cellDeps.push(ccc.CellDep.from({ outPoint: game.dep, depType: "code" }));
        await tx.completeFeeBy(signer, 1000n);
        assertOnlyInputs(tx, inputs.map((c) => c.outPoint));
        const hash = await signer.sendTransaction(tx);
        console.log(`    broadcast ${hash} — waiting for commit…`);
        await waitLive(client, { txHash: hash, index: "0x0" }, "game cell", onTick);
        console.log(`    ✓ game cell live`);
      }
    }
  } else if (!send) {
    console.log(`  ~ game cell: checked after the game script deploys (re-run to see)`);
  }

  // ── 3. the controller account cell ─────────────────────────────────────────
  let account;
  try {
    account = await accountLock(ctx);
  } catch (e) {
    console.log(`  ~ account: ${e.message}`);
  }
  if (account) {
    let cells = 0,
      total = 0n;
    for await (const c of client.findCellsByLock(account.lock, null, false)) {
      cells++;
      total += c.cellOutput.capacity;
    }
    if (cells === 1) {
      console.log(`  = account: live, ${fmtCkb(total)} — skipped`);
    } else if (cells > 1) {
      console.log(`  ! account: ${cells} cells — run \`ckb-controller account drain --send\``);
    } else {
      console.log(`  + account: create with ${accountCkb} CKB at ${account.address.slice(0, 30)}…`);
      if (send) {
        const capacity = accountCkb * CKB;
        const { inputs } = await selectPlainCells(client, keyLock, capacity + ccc.fixedPointFrom("200"));
        const tx = ccc.Transaction.from({ outputs: [{ lock: account.lock, capacity }], outputsData: ["0x"] });
        for (const c of inputs) tx.inputs.push(ccc.CellInput.from({ previousOutput: c.outPoint, since: 0n }));
        await tx.completeFeeBy(signer, 1000n);
        assertOnlyInputs(tx, inputs.map((c) => c.outPoint));
        const hash = await signer.sendTransaction(tx);
        console.log(`    broadcast ${hash} — waiting for commit…`);
        await waitLive(client, { txHash: hash, index: "0x0" }, "account cell", onTick);
        console.log(`    ✓ account live`);
      }
    }
  }
}

export async function run(args, { configPath }) {
  const send = args.includes("send") || args.includes("--send");
  const accountCkb = BigInt((args.find((a) => a.startsWith("--account-ckb=")) ?? "--account-ckb=500").split("=")[1]);
  const gameCellCkb = BigInt((args.find((a) => a.startsWith("--game-ckb=")) ?? "--game-ckb=500").split("=")[1]);

  const ctx = await loadCtx(configPath);
  const { client, signer } = await makeSigner(ctx);
  console.log(`network: ${ctx.network}   rpc: ${ctx.rpc}${send ? "" : "   (DRY RUN — add --send)"}`);
  await ensureDeployed(ctx, client, signer, { send, accountCkb, gameCellCkb });
  console.log(send ? "\ndeploy complete — `ckb-controller status` to verify." : "\nDRY RUN done — re-run with --send to broadcast.");
}
