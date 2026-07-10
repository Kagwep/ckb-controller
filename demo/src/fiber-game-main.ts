// Fiber-central game demo (box #1): the game loop drives the payments.
//
// Same session-funded, budget-capped channel as the buy-troops demo — but
// instead of a manual "buy" button, EVERY SHOT is an off-chain Fiber
// micropayment (rail.pay), and game-over settles the channel on-chain
// (rail.close). Score is engagement; the settled `remoteCkb` is what the game
// earned. Mock by default; ?live=1&peer=…&wss=… runs a real in-browser node.

import { createRail, liveAccountInfo, isMulti, userAccountAddress, type DemoRail, CKB } from "./rail.js";
import { runGame, type GameHandle } from "./fiber-game.js";

const params = new URLSearchParams(location.search);
const DEFAULT_LOCK_CODE_HASH = "0x" + "9c".repeat(32);
const lockCodeHash = params.get("lock") ?? DEFAULT_LOCK_CODE_HASH;
let isLive = params.get("live") === "1"; // set at connect() from the toggle

const COST_PER_SHOT = 1n; // CKB per shot — one off-chain Fiber micropayment
const FIBER_MIN_FUNDING_CKB = 158n; // Fiber's minimum channel funding amount

const $ = (id: string) => document.getElementById(id) as HTMLElement;
const short = (h: string) => (h.length > 22 ? `${h.slice(0, 12)}…${h.slice(-8)}` : h);
const explorerTx = (h: string) => `https://testnet.explorer.nervos.org/transaction/${h}`;

let rail: DemoRail | null = null;
let handle: GameHandle | null = null;
let budgetCkb = 0n;
let score = 0;
let firedShots = 0; // authoritative shot count for the local budget reservation
let payErrors = 0;
const pendingPays = new Set<Promise<unknown>>(); // in-flight optimistic micropayments

function log(msg: string) {
  const li = document.createElement("li");
  li.textContent = msg;
  $("log").prepend(li);
}

// Best-effort human message for any thrown value (fiber-js can reject non-Errors).
function errMsg(e: unknown): string {
  if (e == null) return "(empty/undefined error — see console)";
  if (e instanceof Error) return e.message || e.name || e.toString();
  if (typeof e === "string") return e;
  if (typeof e === "object") {
    const o = e as Record<string, unknown>;
    if (typeof o.message === "string" && o.message) return o.message;
    if (typeof o.error === "string" && o.error) return o.error;
    try {
      return JSON.stringify(e);
    } catch {
      /* fallthrough */
    }
  }
  return String(e);
}

function setTx(el: HTMLElement, hash: string, live: boolean) {
  el.textContent = "";
  if (live && /^0x[0-9a-f]{64}$/i.test(hash)) {
    const a = document.createElement("a");
    a.href = explorerTx(hash);
    a.target = "_blank";
    a.rel = "noopener";
    a.textContent = short(hash);
    el.appendChild(a);
  } else {
    el.textContent = hash ? short(hash) : "(pending)";
  }
}

function refreshHud() {
  if (!rail) return;
  const committed = BigInt(firedShots) * COST_PER_SHOT;
  const remaining = budgetCkb - committed;
  $("score").textContent = String(score);
  $("shots").textContent = String(firedShots);
  $("spent").textContent = String(committed);
  $("remaining").textContent = String(remaining < 0n ? 0n : remaining);
  ($("bar-fill") as HTMLElement).style.width =
    budgetCkb > 0n ? `${Number((committed * 100n) / budgetCkb)}%` : "0%";
}

// Called by the engine on every shot. Optimistic: reserve budget synchronously
// (so rapid fire can't over-commit before pays resolve), then fire the off-chain
// micropayment in the background. The channel's own budget guard is the backstop.
function tryShoot(): boolean {
  if (!rail) return false;
  if ((BigInt(firedShots) + 1n) * COST_PER_SHOT > budgetCkb) return false;
  firedShots++;
  // Track the in-flight pay so settle can wait for it (closing mid-TLC fails).
  const p = rail
    .pay(COST_PER_SHOT)
    .catch((e) => {
      payErrors++;
      console.error("pay failed:", e);
      log(`pay error: ${errMsg(e)}`);
    })
    .finally(() => pendingPays.delete(p));
  pendingPays.add(p);
  refreshHud();
  return true;
}

// Build the effective params from the UI (matching): live toggle + peer fields
// override the URL, so no hand-crafted ?live=1&peer=…&wss=… is needed.
function effectiveParams(): URLSearchParams {
  const p = new URLSearchParams(location.search);
  if (($("live-toggle") as HTMLInputElement).checked) {
    const peer = ($("peer") as HTMLInputElement).value.trim();
    const wss = ($("wss") as HTMLInputElement).value.trim();
    if (!peer || !wss) {
      throw new Error("live mode needs an opponent: fill in the peer pubkey and WSS address");
    }
    p.set("live", "1");
    p.set("peer", peer);
    p.set("wss", wss);
  } else {
    p.delete("live");
  }
  return p;
}

async function connect() {
  const btn = $("connect") as HTMLButtonElement;
  btn.disabled = true;
  try {
    isLive = ($("live-toggle") as HTMLInputElement).checked;
    updateMode(isLive);
    budgetCkb = BigInt(($("budget") as HTMLInputElement).value || "500");
    const eff = effectiveParams();

    // Pre-flight the on-chain account (live only): block a multi-cell account
    // (would fail MultipleInputs) and clamp the budget below the change-cell
    // minimum (would fail CapacityNotEnough) — the two failures hit by hand.
    btn.textContent = isLive ? "Checking account…" : "Opening…";
    const acct = await liveAccountInfo(eff);
    if (acct) {
      if (acct.cellCount === 0) {
        throw new Error("account has no live cells on-chain — fund/deploy it first");
      }
      if (acct.cellCount > 1) {
        throw new Error(
          `account has ${acct.cellCount} cells (must be 1) — run \`node cli/bin.mjs account drain --send\` to merge, then retry`,
        );
      }
      if (acct.maxFundableCkb < FIBER_MIN_FUNDING_CKB) {
        throw new Error(
          `account balance ${acct.cellCapacityCkb} CKB is too small — a channel needs ≥ ${FIBER_MIN_FUNDING_CKB} CKB funding plus ~170 CKB change. Grow it: \`node cli/bin.mjs account grow <ckb> --send\``,
        );
      }
      if (budgetCkb > acct.maxFundableCkb) {
        log(
          `budget ${budgetCkb} > fundable max ${acct.maxFundableCkb} CKB (account ${acct.cellCapacityCkb} CKB − change reserve) — clamped`,
        );
        budgetCkb = acct.maxFundableCkb;
      }
      if (budgetCkb < FIBER_MIN_FUNDING_CKB) {
        log(`budget below Fiber's ${FIBER_MIN_FUNDING_CKB} CKB minimum — raised`);
        budgetCkb = FIBER_MIN_FUNDING_CKB;
      }
      ($("budget") as HTMLInputElement).value = String(budgetCkb);
    }

    btn.textContent = isLive ? "Starting Fiber node…" : "Opening…";
    rail = await createRail(eff, lockCodeHash, budgetCkb);
    const info = await rail.open(budgetCkb);

    $("addr").textContent = rail.address;
    $("sess").textContent = short(rail.sessionLabel);
    setTx($("fundtx"), info.id, rail.mode === "live");
    $("connect-out").classList.remove("hidden");
    $("budget-total").textContent = String(budgetCkb);

    if (rail.mode === "live") {
      btn.textContent = "Waiting for channel (~90s)…";
      log(`[live] funding committed (${short(info.id)}) · waiting for channel to become routable…`);
    }
    await rail.waitReady(COST_PER_SHOT);

    refreshHud();
    log(`[${rail.mode}] channel ready · budget ${budgetCkb} CKB · aim with mouse, fire with click/Space`);

    // Start the game: every shot -> tryShoot -> off-chain micropayment.
    $("stage-wrap").classList.remove("disabled");
    ($("endgame") as HTMLButtonElement).disabled = false;
    btn.textContent = "Playing ✓";
    handle = runGame({
      canvas: $("stage") as HTMLCanvasElement,
      tryShoot,
      onScore: (s) => {
        score = s;
        refreshHud();
      },
      onOver,
    });
  } catch (e) {
    console.error("connect failed:", e);
    btn.disabled = false;
    btn.textContent = "Approve session & open channel";
    log(`error: ${withHint(errMsg(e))}`);
  }
}

// Translate the raw lock/fiber errors into actionable guidance.
function withHint(m: string): string {
  if (/MultipleInputs|error code 5|#5\b/.test(m) || m.includes("9d3ce3e2")) {
    return `${m}  →  account has >1 cell; run \`node cli/bin.mjs account drain --send\``;
  }
  if (/change cell|CapacityNotEnough|capacity not enough/i.test(m)) {
    return `${m}  →  budget too large for the account balance; lower it`;
  }
  if (/funding amount|greater than or equal/i.test(m)) {
    return `${m}  →  channel below Fiber's ${FIBER_MIN_FUNDING_CKB} CKB minimum; raise the budget (grow the account if needed)`;
  }
  return m;
}

function onOver(result: { score: number; reason: string }) {
  handle = null;
  ($("endgame") as HTMLButtonElement).disabled = true;
  $("stage-wrap").classList.add("disabled");
  $("finalscore").textContent = String(result.score);
  $("settle-card").classList.remove("disabled");
  ($("settle") as HTMLButtonElement).disabled = false;
  log(`game over (${result.reason}) · score ${result.score} · ${firedShots} shots · settle to finish`);
}

// Close the channel, retrying while the node is still clearing in-flight TLCs
// (a shutdown during a pending TLC fails with InvalidState / "pending outbound tlcs").
async function closeWithRetry(r: DemoRail, attempts = 6) {
  for (let i = 0; ; i++) {
    try {
      return await r.close();
    } catch (e) {
      const m = errMsg(e);
      if (i < attempts && /pending outbound tlcs|invalid state/i.test(m)) {
        log(`settle retry ${i + 1}/${attempts} — channel still clearing payments…`);
        await new Promise((res) => setTimeout(res, 1500));
        continue;
      }
      throw e;
    }
  }
}

async function settle() {
  if (!rail) return;
  const btn = $("settle") as HTMLButtonElement;
  btn.disabled = true;
  btn.textContent = "Settling…";
  try {
    // Let any in-flight micropayments finish before closing.
    if (pendingPays.size) {
      log(`waiting for ${pendingPays.size} in-flight payment(s) to settle…`);
      await Promise.allSettled([...pendingPays]);
    }
    const info = await closeWithRetry(rail);
    $("local").textContent = `${info.localCkb} CKB`;
    $("remote").textContent = `${info.remoteCkb} CKB`;
    setTx($("settletx"), info.settleTxHash, rail.mode === "live");
    $("settle-out").classList.remove("hidden");
    btn.textContent = "Settled ✓";
    log(`[${rail.mode}] channel closed · ${info.remoteCkb} CKB to game, ${info.localCkb} CKB back to account · 1 L1 settle`);
    if (payErrors > 0) log(`note: ${payErrors} background pay error(s) during play — see console`);
  } catch (e) {
    console.error("settle failed:", e);
    btn.disabled = false;
    btn.textContent = "Settle on-chain";
    log(`settle error: ${errMsg(e)}`);
  }
}

// Reflect mock/live in the badge and reveal the opponent fields only when live.
function updateMode(live: boolean) {
  const el = $("mode");
  el.textContent = live
    ? "mode: LIVE (in-browser Fiber node)"
    : "mode: mock (in-browser, no node)";
  el.classList.toggle("live", live);
  $("peer-fields").classList.toggle("hidden", !live);
}

$("connect").addEventListener("click", connect);
$("endgame").addEventListener("click", () => handle?.stop());
$("settle").addEventListener("click", settle);
$("live-toggle").addEventListener("change", (e) =>
  updateMode((e.target as HTMLInputElement).checked),
);

// Prefill the matching fields from the URL (?live=1&peer=…&wss=…), so a link
// still works — but the fields, not the URL, are the source of truth.
(($("live-toggle") as HTMLInputElement).checked = isLive);
if (params.get("peer")) ($("peer") as HTMLInputElement).value = params.get("peer") as string;
if (params.get("wss")) ($("wss") as HTMLInputElement).value = params.get("wss") as string;
updateMode(isLive);

log(
  `ready · toggle "Play live on testnet" to use a Fiber peer · ${COST_PER_SHOT} CKB/shot · 1 CKB = ${CKB} shannons`,
);

// Multi-user mode (?multi=1): show THIS browser's own controller account address
// (persisted per-browser keypair) so two browsers are visibly two identities.
if (isMulti(params)) {
  userAccountAddress(params).then((addr) => {
    if (!addr) return;
    const el = $("user-addr");
    el.textContent = `this browser: ${short(addr)}`;
    el.title = addr;
    el.classList.remove("hidden");
    log(`multi-user · this browser's account ${short(addr)}`);
  });
}
