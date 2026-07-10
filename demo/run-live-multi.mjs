// Phase 2 driver: drive TWO live browsers end-to-end, headless — A pays B
// through the hub. Each browser gets its OWN persistent profile (its own
// localStorage → its own per-user identity AND its own Fiber IndexedDB channel
// state; sharing a profile would collapse them into one user). Flow:
//   both load ?live=1&multi=1&peer=…&wss=… → both open controller-funded
//   channels to the hub → B requests an invoice → A pays it with
//   trampoline_hops=[hub] → assert B's page shows the amount received →
//   both settle. Extends the run-live.mjs single-user pattern.
// usage: node run-live-multi.mjs <hubPubkey> <wssMultiaddr> [budgetCkb] [payCkb]
// env: DEMO_URL (default http://localhost:5174), PROFILE_DIR_A, PROFILE_DIR_B,
//      EDGE_PATH. Pre-reqs: both per-user accounts funded (fund-user.mjs), hub
//      up with acceptor liquidity (docs/internals/phase2-live-run.md).
import puppeteer from "puppeteer-core";

const [PEER, WSS] = [process.argv[2], process.argv[3]];
const BUDGET = process.argv[4] ?? "300";
const PAY = process.argv[5] ?? "20";
if (!PEER || !WSS) throw new Error("usage: node run-live-multi.mjs <hubPubkey> <wssMultiaddr> [budgetCkb] [payCkb]");

const BASE = process.env.DEMO_URL ?? "http://localhost:5174";
const EDGE = process.env.EDGE_PATH ?? "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const url = `${BASE}/?live=1&multi=1&peer=${PEER}&wss=${encodeURIComponent(WSS)}`;

const stage = (s) => console.log(`\n══ ${s} ══`);

// One user = one browser with a persistent, PER-USER profile (identity + open
// channel state live in it — losing it orphans channel funds until force-close).
async function launchUser(name, profileDir) {
  const browser = await puppeteer.launch({
    executablePath: EDGE,
    headless: "new",
    userDataDir: profileDir,
    args: ["--no-first-run", "--disable-features=msEdgeIdentityFeatures"],
  });
  const page = await browser.newPage();
  page.setDefaultTimeout(300000);
  page.on("console", (m) => console.log(`[${name}:console:${m.type()}]`, m.text()));
  page.on("pageerror", (e) => console.log(`[${name}:pageerror]`, e.message));
  const text = (sel) => page.$eval(sel, (el) => el.textContent ?? "");
  const latestLog = () => page.$eval("#log li", (el) => el.textContent ?? "").catch(() => "");
  async function waitFor(pred, what, timeoutMs) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      if (await pred().catch(() => false)) return;
      if (Date.now() > deadline) throw new Error(`[${name}] timeout waiting for ${what} (last log: ${await latestLog()})`);
      await new Promise((r) => setTimeout(r, 2000));
    }
  }
  return { name, browser, page, text, waitFor };
}

// Connect one user: budget → connect → session-signed external funding →
// ChannelReady (~2-3 min: L1 commit + channel handshake).
async function connect(u) {
  await u.page.goto(url, { waitUntil: "networkidle2" });
  const iso = await u.page.evaluate(() => self.crossOriginIsolated);
  console.log(`[${u.name}] crossOriginIsolated:`, iso);
  await u.page.$eval("#budget", (el, v) => (el.value = v), BUDGET);
  await u.page.click("#connect");
  console.log(`[${u.name}] clicked connect (budget ${BUDGET} CKB)…`);
  await u.waitFor(async () => (await u.text("#connect")).includes("Channel ready"), "channel ready", 300000);
  const addr = await u.text("#addr");
  console.log(`[${u.name}] CHANNEL READY · account ${addr} · funding tx ${await u.text("#fundtx")}`);
  return addr;
}

async function settleUser(u) {
  await u.page.click("#settle");
  await u.waitFor(async () => (await u.text("#settle")).includes("Settled"), "settle", 300000);
  console.log(`[${u.name}] SETTLED · local ${await u.text("#local")} · remote ${await u.text("#remote")} · tx ${await u.text("#settletx")}`);
}

stage("launch A + B (separate persistent profiles)");
console.log("navigating:", url);
const a = await launchUser("A", process.env.PROFILE_DIR_A ?? "D:/projects/ckb-controller/demo/.edge-profile-userA");
const b = await launchUser("B", process.env.PROFILE_DIR_B ?? "D:/projects/ckb-controller/demo/.edge-profile-userB");

try {
  stage("open channels to the hub (both users, in parallel)");
  const [addrA, addrB] = await Promise.all([connect(a), connect(b)]);
  if (addrA === addrB) throw new Error("A and B derived the SAME account — profiles are not isolated (check PROFILE_DIR_A/B)");

  stage(`B requests an invoice for ${PAY} CKB`);
  await b.page.$eval("#req-amount", (el, v) => (el.value = v), PAY);
  await b.page.click("#req-btn");
  await b.waitFor(async () => (await b.text("#invoice-str")).length > 0, "invoice string", 60000);
  const invoice = await b.text("#invoice-str");
  console.log(`[B] invoice: ${invoice}`);

  stage("A pays the invoice via the hub (trampoline)");
  await a.page.$eval("#pay-invoice", (el, v) => (el.value = v), invoice);
  await a.page.click("#pay-btn");
  await a.waitFor(async () => (await a.text("#pay-status")).includes("paid"), "A's payment success", 180000);
  console.log(`[A] paid · spent ${await a.text("#spent")} CKB of ${await a.text("#budget-total")}`);

  stage("assert B received the payment");
  await b.waitFor(async () => (await b.text("#recv-status")).includes("PAID"), "B's invoice PAID", 180000);
  const recv = await b.text("#recv-ckb");
  if (BigInt(recv) !== BigInt(PAY)) throw new Error(`B received ${recv} CKB, expected ${PAY}`);
  console.log(`[B] received ${recv} CKB ✓ — A → hub → B routed`);

  stage("settle both channels");
  await Promise.all([settleUser(a), settleUser(b)]);

  stage("PASS — two-user trampoline payment complete");
} finally {
  await Promise.allSettled([a.browser.close(), b.browser.close()]);
}
