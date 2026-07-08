// Multiplayer scoreboard demo — the aggregator client (the "N tabs = N players"
// half of the game-cell design), now a thin UI over @ckb-controller/sdk's
// GameClient/GamePlayer. Each browser TAB is one player with its own session
// key; a move is a session-signed INTENT posted to the operator, which batches
// intents into a shared game-cell transition — the operator sequences
// (liveness), the on-chain type script enforces the rules (safety).
//
// Open several tabs (optionally ?operator=<url>&game=0x<32 bytes>) and watch
// every tab's moves land on the same board. Run the operator first:
//   cargo run -p paymaster-service --bin game-operator   (defaults to :9944)

import init, * as wasmModule from "../pkg/controller.js";
import { Controller, type Board, type ControllerConfig, type ControllerWasm } from "@ckb-controller/sdk";
import { CONFIG, MANIFEST } from "./config.js";

const params = new URLSearchParams(location.search);
const POINTS_PER_MOVE = 5n;

const short = (h: string) => (h.length > 14 ? `${h.slice(0, 8)}…${h.slice(-4)}` : h);
const $ = (id: string) => document.getElementById(id) as HTMLElement;
function log(msg: string) {
  const li = document.createElement("li");
  li.textContent = msg;
  $("log").prepend(li);
}

function renderBoard(board: Board | null, playerHash: string) {
  const tbody = $("board");
  tbody.innerHTML = "";
  if (!board) {
    $("seq").textContent = "operator offline";
    return;
  }
  $("seq").textContent = `seq ${board.seq}`;
  const rows = [...board.players].sort((a, b) => b.score - a.score);
  for (const p of rows) {
    const tr = document.createElement("tr");
    const me = p.hash.toLowerCase() === playerHash.toLowerCase();
    tr.innerHTML =
      `<td>${me ? "▶ " : ""}${short(p.hash)}${me ? " (you)" : ""}</td>` +
      `<td class="num">${p.score}</td><td class="num">${p.nonce}</td>`;
    if (me) tr.classList.add("me");
    tbody.appendChild(tr);
  }
}

async function boot() {
  await init();
  const controller = Controller.load({
    config: CONFIG as unknown as ControllerConfig,
    manifest: MANIFEST,
    wasm: wasmModule as unknown as ControllerWasm,
  });
  const game = controller.game(params.get("operator") ?? undefined, params.get("game") ?? undefined);
  const player = game.player(); // this tab's identity: a fresh session key

  $("you").textContent = short(player.hash);
  $("operator").textContent = game.operatorUrl;
  $("game").textContent = short(game.gameId);

  const refresh = async () => renderBoard(await game.board(), player.hash);

  ($("score") as HTMLButtonElement).addEventListener("click", async () => {
    const btn = $("score") as HTMLButtonElement;
    btn.disabled = true;
    try {
      const r = await player.move(POINTS_PER_MOVE);
      log(`+${POINTS_PER_MOVE} scored · seq ${r.seq} · tx ${short(r.txHash)} · 0 popups`);
    } catch (e) {
      log(`move rejected: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      await refresh();
      btn.disabled = false;
    }
  });

  await refresh();
  // Poll the shared board so other tabs' moves show up here too.
  setInterval(refresh, 1500);
  log(`ready · you are ${short(player.hash)} · operator ${game.operatorUrl}`);
}

boot();
