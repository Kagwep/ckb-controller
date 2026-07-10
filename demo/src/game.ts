// Multiplayer MATCH demo — the two rails in one loop (Phase 3). Each browser TAB
// is one player; a move is a session-signed INTENT posted to the operator, which
// batches intents into a shared game-cell transition (state rail — operator
// sequences, the on-chain type script enforces the rules). On top of that, a
// simple VALUE rule: scoring publishes a "bounty" invoice via the operator's
// non-custodial relay, and any other player can pay it over a Fiber channel
// (value rail — player↔hub↔player TLCs; the operator only relays the string).
//
// Identity: in ?multi=1 the board player reuses THIS browser's per-user session
// key (userKeys.ts) so board identity == paying identity; otherwise a fresh
// per-tab key. Value: mock by default; ?live=1&peer=&wss= for a real channel.
//
//   cargo run -p paymaster-service --bin game-operator     # operator (:9944)
//   npm run dev                                            # then open game.html
//   ...&operator=<url>&game=0x<32B>   ?multi=1   ?live=1&peer=&wss=
import init, * as wasmModule from "../pkg/controller.js";
import {
  Controller,
  hx,
  type Board,
  type ControllerConfig,
  type ControllerWasm,
  type GamePlayer,
  type ResultEvent,
} from "@ckb-controller/sdk";
import { CONFIG, MANIFEST } from "./config.js";
import { getUserKeys } from "./userKeys.js";
import { createRail, type DemoRail } from "./rail.js";

const params = new URLSearchParams(location.search);
const POINTS_PER_MOVE = 5n;
const BOUNTY_CKB = 5n; // scoring publishes a bounty invoice for this much CKB
const MAX_TRAMPOLINE_FEE_CKB = 10n; // generous — fee estimation with gossip off is rough

const multi = params.get("multi") === "1";
const isLive = params.get("live") === "1";
const DEFAULT_LOCK_CODE_HASH = "0x" + "9c".repeat(32);
const lockCodeHash = params.get("lock") ?? DEFAULT_LOCK_CODE_HASH;

const short = (h: string) => (h.length > 14 ? `${h.slice(0, 8)}…${h.slice(-4)}` : h);
const $ = (id: string) => document.getElementById(id) as HTMLElement;
function log(msg: string) {
  const li = document.createElement("li");
  li.textContent = msg;
  $("log").prepend(li);
}
function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message || e.name;
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
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

function renderResults(events: ResultEvent[]) {
  const ul = $("results");
  ul.innerHTML = "";
  for (const e of [...events].reverse()) {
    const li = document.createElement("li");
    if (e.kind === "score") {
      li.textContent = `⚑ ${short(String(e.player))} +${e.points} · seq ${e.seq}`;
    } else if (e.kind === "invoice_published") {
      li.textContent = `📩 bounty ${e.amount_ckb} CKB from ${short(String(e.from ?? "?"))}`;
    } else if (e.kind === "invoice_paid") {
      li.textContent = `💰 bounty #${e.id} paid → ${short(String(e.from ?? "?"))} (${e.amount_ckb} CKB)`;
    } else {
      li.textContent = JSON.stringify(e);
    }
    ul.appendChild(li);
  }
}

async function boot() {
  await init();
  const wasm = wasmModule as unknown as ControllerWasm;
  const controller = Controller.load({
    config: CONFIG as unknown as ControllerConfig,
    manifest: MANIFEST,
    wasm,
    keys: multi ? getUserKeys() : undefined,
  });
  const game = controller.game(params.get("operator") ?? undefined, params.get("game") ?? undefined);
  // Board identity: in multi mode reuse the per-user session key so the board
  // player == the paying identity; otherwise a fresh per-tab key.
  const player: GamePlayer = game.player(multi ? hx(getUserKeys().session.priv) : undefined);

  $("you").textContent = short(player.hash);
  $("operator").textContent = game.operatorUrl;
  $("game").textContent = short(game.gameId);

  // Multi-device: surface THIS device's account address with tap-to-copy (no
  // hover on phones), so it can be funded before "Open channel".
  if (multi) {
    const addr = controller.account.address;
    $("user-addr").textContent = short(addr);
    $("user-addr").title = addr;
    $("addr-row").classList.remove("hidden");
    ($("copy-addr") as HTMLButtonElement).addEventListener("click", async () => {
      const btn = $("copy-addr") as HTMLButtonElement;
      try {
        await navigator.clipboard.writeText(addr);
        btn.textContent = "copied ✓";
      } catch {
        prompt("account address (copy):", addr); // clipboard denied — fall back
        btn.textContent = "copy address";
        return;
      }
      setTimeout(() => (btn.textContent = "copy address"), 1500);
    });
    log(`multi-user · this device's account ${short(addr)} — fund it, then Open channel`);
  }

  const refresh = async () => {
    const board = await game.board();
    player.syncNonce(board);
    renderBoard(board, player.hash);
    renderResults(await game.results(20));
  };

  // --- state rail: score a move (session-signed intent) ---------------------
  // On a successful score, publish a bounty invoice (value rail) another player
  // can pay. In live mode the rail must be open (only its Fiber node holds the
  // preimage); in mock the same page's rail issues + self-registers it.
  let rail: DemoRail | null = null;

  ($("score") as HTMLButtonElement).addEventListener("click", async () => {
    const btn = $("score") as HTMLButtonElement;
    btn.disabled = true;
    try {
      const r = await player.move(POINTS_PER_MOVE);
      log(`+${POINTS_PER_MOVE} scored · seq ${r.seq} · tx ${short(r.txHash)} · 0 popups`);
      if (rail) {
        try {
          const invoice = await rail.newInvoice(BOUNTY_CKB, "controller-demo bounty");
          const { id } = await game.publishInvoice(invoice, BOUNTY_CKB, { from: player.hash });
          log(`published bounty #${id} for ${BOUNTY_CKB} CKB — another player can pay it`);
        } catch (e) {
          log(`bounty publish skipped: ${errMsg(e)}`);
        }
      }
    } catch (e) {
      log(`move rejected: ${errMsg(e)}`);
    } finally {
      await refresh();
      btn.disabled = false;
    }
  });

  // --- value rail: open a channel to the hub --------------------------------
  ($("open-channel") as HTMLButtonElement).addEventListener("click", async () => {
    const btn = $("open-channel") as HTMLButtonElement;
    btn.disabled = true;
    btn.textContent = isLive ? "Starting Fiber node…" : "Opening…";
    try {
      const budget = BigInt(($("budget") as HTMLInputElement).value || "300");
      rail = await createRail(params, lockCodeHash, budget);
      const info = await rail.open(budget);
      $("chan-addr").textContent = short(rail.address);
      $("chan-fundtx").textContent = short(info.id);
      if (rail.mode === "live") {
        btn.textContent = "Waiting for channel (~90s)…";
        log(`[live] funding committed (${short(info.id)}) · waiting to become routable…`);
      }
      await rail.waitReady(BOUNTY_CKB);
      ($("pay-bounty") as HTMLButtonElement).disabled = false;
      ($("settle") as HTMLButtonElement).disabled = false;
      btn.textContent = "Channel ready ✓";
      refreshChannel();
      log(`[${rail.mode}] channel ready · budget ${budget} CKB`);
    } catch (e) {
      btn.disabled = false;
      btn.textContent = "Open channel";
      log(`channel error: ${errMsg(e)}`);
    }
  });

  function refreshChannel() {
    if (!rail) return;
    $("chan-spent").textContent = String(rail.spentCkb());
    $("chan-remaining").textContent = String(rail.remainingCkb());
  }

  // --- value rail: pay the next bounty via the relay + trampoline -----------
  ($("pay-bounty") as HTMLButtonElement).addEventListener("click", async () => {
    if (!rail) return;
    const btn = $("pay-bounty") as HTMLButtonElement;
    btn.disabled = true;
    $("bounty-status").textContent = "finding a bounty…";
    try {
      // Live: pass our hash so the relay skips our own invoices and honours
      // addressing. Mock (single page): omit it so we can pay a bounty we issued.
      const inv = await game.nextInvoice(isLive ? player.hash : undefined);
      if (!inv) {
        $("bounty-status").textContent = "no unpaid bounty";
        log("no unpaid bounty in the relay yet — score first");
        return;
      }
      $("bounty-status").textContent = `paying bounty #${inv.id} (${inv.amountCkb} CKB)…`;
      const hops = isLive ? [params.get("peer") as string] : undefined;
      await rail.payInvoice(inv.invoice, { trampolineHops: hops, maxFeeCkb: MAX_TRAMPOLINE_FEE_CKB });
      await game.markPaid(inv.id);
      $("bounty-status").textContent = `paid bounty #${inv.id} ✓`;
      log(`paid bounty #${inv.id} (${inv.amountCkb} CKB) via ${isLive ? "hub (trampoline)" : "mock"}`);
      refreshChannel();
    } catch (e) {
      $("bounty-status").textContent = "failed";
      log(`pay bounty error: ${errMsg(e)}`);
    } finally {
      await refresh();
      btn.disabled = false;
    }
  });

  // --- value rail: settle the channel ---------------------------------------
  ($("settle") as HTMLButtonElement).addEventListener("click", async () => {
    if (!rail) return;
    const btn = $("settle") as HTMLButtonElement;
    btn.disabled = true;
    btn.textContent = "Settling…";
    try {
      const info = await rail.close();
      $("chan-settle").textContent = `local ${info.localCkb} · remote ${info.remoteCkb} CKB · tx ${short(info.settleTxHash)}`;
      ($("pay-bounty") as HTMLButtonElement).disabled = true;
      btn.textContent = "Settled ✓";
      log(`[${rail.mode}] channel settled · ${info.remoteCkb} CKB out, ${info.localCkb} CKB back`);
    } catch (e) {
      btn.disabled = false;
      btn.textContent = "Settle on-chain";
      log(`settle error: ${errMsg(e)}`);
    }
  });

  await refresh();
  setInterval(refresh, 1500);
  log(`ready · you are ${short(player.hash)} · ${multi ? "multi-user" : "single"} · value ${isLive ? "LIVE" : "mock"}`);
}

boot();
