import { CKB } from "./rail.js";
import { createRail, isMulti, userAccountAddress, type DemoRail } from "./rail.js";

const params = new URLSearchParams(location.search);

// Demo lock code hash. In mock mode this only flavours the derived address; set it
// to your deployed testnet lock's code hash (?lock=0x…) to show the real address.
// In live mode (?live=1) the deployed account is reconstructed and this is ignored.
const DEFAULT_LOCK_CODE_HASH = "0x" + "9c".repeat(32);
const lockCodeHash = params.get("lock") ?? DEFAULT_LOCK_CODE_HASH;
const isLive = params.get("live") === "1";

const COST_PER_TROOP = 5n; // CKB per troop

const $ = (id: string) => document.getElementById(id) as HTMLElement;
const short = (h: string) => (h.length > 22 ? `${h.slice(0, 12)}…${h.slice(-8)}` : h);

const explorerTx = (h: string) => `https://testnet.explorer.nervos.org/transaction/${h}`;
// Render a tx hash into `el`: a clickable explorer link for real on-chain hashes
// (live mode), plain text otherwise (mock hashes don't resolve on the explorer).
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

let rail: DemoRail | null = null;
let troops = 0;
let pays = 0;
let budgetCkb = 0n;

function log(msg: string) {
  const li = document.createElement("li");
  li.textContent = msg;
  $("log").prepend(li);
}

// Best-effort human message for any thrown value (fiber-js can reject with non-Errors).
function errMsg(e: unknown): string {
  if (e == null) return "(empty/undefined error — see console)";
  if (e instanceof Error) return e.message || e.name || e.toString();
  if (typeof e === "string") return e;
  if (typeof e === "object") {
    const o = e as Record<string, unknown>;
    if (typeof o.message === "string" && o.message) return o.message;
    if (typeof o.error === "string" && o.error) return o.error;
    try { return JSON.stringify(e); } catch { /* fallthrough */ }
  }
  return String(e);
}

function refresh() {
  if (!rail) return;
  const spent = rail.spentCkb();
  const remaining = rail.remainingCkb();
  $("troops").textContent = String(troops);
  $("spent").textContent = String(spent);
  $("remaining").textContent = String(remaining);
  $("paycount").textContent = `${pays} off-chain pays`;
  ($("bar-fill") as HTMLElement).style.width =
    budgetCkb > 0n ? `${Number((spent * 100n) / budgetCkb)}%` : "0%";

  const broke = remaining < COST_PER_TROOP;
  ($("buy1") as HTMLButtonElement).disabled = broke;
  ($("buy10") as HTMLButtonElement).disabled = remaining < COST_PER_TROOP * 10n;
}

async function connect() {
  const btn = $("connect") as HTMLButtonElement;
  btn.disabled = true;
  btn.textContent = isLive ? "Starting Fiber node…" : "Opening…";
  try {
    budgetCkb = BigInt(($("budget") as HTMLInputElement).value || "500");
    rail = await createRail(params, lockCodeHash, budgetCkb);
    const info = await rail.open(budgetCkb);

    $("addr").textContent = rail.address;
    $("sess").textContent = short(rail.sessionLabel);
    setTx($("fundtx"), info.id, rail.mode === "live");
    $("signedby").innerHTML = `<strong>${info.signedBy}</strong>`;
    $("connect-out").classList.remove("hidden");

    $("budget-total").textContent = String(budgetCkb);

    // Gate buys on the channel actually being routable. In live mode the funding
    // tx is committed but the channel isn't ready to route for ~90s — buying now
    // fails with "max outbound liquidity 0" (the "first buy failed" bug). Wait for
    // ChannelReady + outbound liquidity before enabling the buy buttons. Instant
    // in mock mode.
    if (rail.mode === "live") {
      btn.textContent = "Waiting for channel to be ready (~90s)…";
      log(`[live] funding committed (${short(info.id)}) · waiting for channel to become routable…`);
    }
    await rail.waitReady(COST_PER_TROOP);

    refresh();
    $("play-card").classList.remove("disabled");
    $("p2p-card").classList.remove("disabled");
    ($("req-btn") as HTMLButtonElement).disabled = false;
    ($("pay-btn") as HTMLButtonElement).disabled = false;
    $("settle-card").classList.remove("disabled");
    ($("settle") as HTMLButtonElement).disabled = false;
    btn.textContent = "Channel ready ✓";
    log(`[${rail.mode}] channel ready · budget ${budgetCkb} CKB · ${short(info.id)}`);
  } catch (e) {
    console.error("connect failed:", e);
    btn.disabled = false;
    btn.textContent = "Approve session & open channel";
    log(`error: ${errMsg(e)}`);
  }
}

async function buy(n: number) {
  if (!rail) return;
  const buyBtns = [$("buy1"), $("buy10")] as HTMLButtonElement[];
  buyBtns.forEach((b) => (b.disabled = true));
  let bought = 0;
  try {
    for (let i = 0; i < n; i++) {
      if (rail.remainingCkb() < COST_PER_TROOP) break;
      await rail.pay(COST_PER_TROOP);
      troops++;
      pays++;
      bought++;
    }
    log(`bought ${bought} troop${bought === 1 ? "" : "s"} · ${bought * Number(COST_PER_TROOP)} CKB off-chain · 0 gas`);
  } catch (e) {
    console.error("pay failed:", e);
    log(`pay error: ${errMsg(e)}`);
  }
  refresh();
}

// Trampoline routing fee ceiling. Generous on purpose: the browser node runs
// with gossip off, so fee estimation is rough (see docs/internals/phase2-live-run.md).
const MAX_TRAMPOLINE_FEE_CKB = 10n;

let receivedCkb = 0n;

// Receive side: issue an invoice, show it to copy, then wait for the payer.
async function requestInvoice() {
  if (!rail) return;
  const btn = $("req-btn") as HTMLButtonElement;
  btn.disabled = true;
  try {
    const amount = BigInt(($("req-amount") as HTMLInputElement).value || "20");
    const invoice = await rail.newInvoice(amount, "controller-demo p2p");
    $("invoice-str").textContent = invoice; // full string — the payer pastes it
    $("recv-status").textContent = "waiting for payment…";
    $("invoice-out").classList.remove("hidden");
    log(`invoice created for ${amount} CKB — send it to the payer`);
    await rail.waitInvoicePaid(invoice);
    receivedCkb += amount;
    $("recv-status").textContent = "PAID ✓";
    $("recv-ckb").textContent = String(receivedCkb);
    log(`invoice paid ✓ received ${amount} CKB (total ${receivedCkb})`);
  } catch (e) {
    console.error("invoice failed:", e);
    $("recv-status").textContent = "failed";
    log(`invoice error: ${errMsg(e)}`);
  }
  btn.disabled = false;
}

// Pay side: paste the other player's invoice; live routes via the hub (the
// channel peer) as the single trampoline hop — the browser has no graph.
async function payViaHub() {
  if (!rail) return;
  const invoice = ($("pay-invoice") as HTMLInputElement).value.trim();
  if (!invoice) {
    log("paste an invoice first");
    return;
  }
  const btn = $("pay-btn") as HTMLButtonElement;
  btn.disabled = true;
  $("pay-status").textContent = "paying…";
  try {
    const hops = isLive ? [params.get("peer") as string] : undefined;
    await rail.payInvoice(invoice, { trampolineHops: hops, maxFeeCkb: MAX_TRAMPOLINE_FEE_CKB });
    $("pay-status").textContent = "paid ✓";
    log(`invoice paid via ${isLive ? "hub (trampoline)" : "mock registry"} · total spent ${rail.spentCkb()} CKB`);
    refresh();
  } catch (e) {
    console.error("p2p pay failed:", e);
    $("pay-status").textContent = "failed";
    log(`p2p pay error: ${errMsg(e)}`);
  }
  btn.disabled = false;
}

async function settle() {
  if (!rail) return;
  const btn = $("settle") as HTMLButtonElement;
  btn.disabled = true;
  btn.textContent = "Settling…";
  try {
    const info = await rail.close();
    $("local").textContent = `${info.localCkb} CKB`;
    $("remote").textContent = `${info.remoteCkb} CKB`;
    setTx($("settletx"), info.settleTxHash, rail.mode === "live");
    $("settle-out").classList.remove("hidden");

    ($("buy1") as HTMLButtonElement).disabled = true;
    ($("buy10") as HTMLButtonElement).disabled = true;
    $("play-card").classList.add("disabled");
    btn.textContent = "Settled ✓";
    log(`[${rail.mode}] channel closed · ${info.remoteCkb} CKB to game, ${info.localCkb} CKB back to account · 1 L1 settle`);
  } catch (e) {
    console.error("settle failed:", e);
    btn.disabled = false;
    btn.textContent = "Settle on-chain";
    log(`settle error: ${errMsg(e)}`);
  }
}

$("connect").addEventListener("click", connect);
$("buy1").addEventListener("click", () => buy(1));
$("buy10").addEventListener("click", () => buy(10));
$("req-btn").addEventListener("click", requestInvoice);
$("pay-btn").addEventListener("click", payViaHub);
$("settle").addEventListener("click", settle);

// reflect the real mode in the badge (was hardcoded to "mock" in index.html)
const modeEl = $("mode");
modeEl.textContent = isLive
  ? "mode: LIVE (in-browser Fiber node)"
  : "mode: mock (in-browser, no node)";
modeEl.classList.toggle("live", isLive);

// surface the mode + lock code hash for clarity
log(`ready · mode ${isLive ? "LIVE (in-browser Fiber)" : "mock"} · lock ${short(lockCodeHash)} · 1 CKB = ${CKB} shannons`);

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
