// Phase 3 driver: drive TWO live browsers through a full MATCH — state + value
// in one loop — headless. Extends run-live-multi.mjs (same per-user profiles),
// but targets game.html (the multiplayer match page) instead of index.html.
// Flow:
//   both load game.html?live=1&multi=1&peer=…&wss=…&operator=… → both OPEN
//   controller-funded channels to the hub → both SCORE (session-signed intents;
//   the shared board must show both players) → scoring auto-publishes a "bounty"
//   invoice via the operator's non-custodial relay → A pays the next bounty
//   (B's) via trampoline_hops=[hub] + the relay → assert it settled (operator
//   /results shows invoice_paid) → both SETTLE → dump GET /results.
//
// usage: node run-live-match.mjs <hubPubkey> <wssMultiaddr> [budgetCkb]
// env: DEMO_URL (default http://localhost:5173), OPERATOR_URL (default
//      http://127.0.0.1:9944), PROFILE_DIR_A/B, EDGE_PATH.
// Pre-reqs (see docs/internals/phase2-live-run.md §"Phase 3 match run"):
//   - game-operator running (CHAIN=mock for a dry match, CHAIN=http for on-chain
//     state) and reachable at OPERATOR_URL;
//   - hub up with acceptor liquidity; both per-user accounts funded AND SINGLE-
//     CELL (drain first — see the drain caveat in the runbook).
import puppeteer from "puppeteer-core";

const [PEER, WSS] = [process.argv[2], process.argv[3]];
const BUDGET = process.argv[4] ?? "300";
if (!PEER || !WSS) throw new Error("usage: node run-live-match.mjs <hubPubkey> <wssMultiaddr> [budgetCkb]");

const BASE = process.env.DEMO_URL ?? "http://localhost:5173";
const OPERATOR = process.env.OPERATOR_URL ?? "http://127.0.0.1:9944";
const EDGE = process.env.EDGE_PATH ?? "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const url =
  `${BASE}/game.html?live=1&multi=1&peer=${PEER}` +
  `&wss=${encodeURIComponent(WSS)}&operator=${encodeURIComponent(OPERATOR)}`;

const stage = (s) => console.log(`\n══ ${s} ══`);

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

// Open one user's channel to the hub (budget → open → ChannelReady, ~2–3 min).
async function openChannel(u) {
  await u.page.goto(url, { waitUntil: "networkidle2" });
  const iso = await u.page.evaluate(() => self.crossOriginIsolated);
  console.log(`[${u.name}] crossOriginIsolated:`, iso, "· you:", await u.text("#you"));
  await u.page.$eval("#budget", (el, v) => (el.value = v), BUDGET);
  await u.page.click("#open-channel");
  console.log(`[${u.name}] opening channel (budget ${BUDGET} CKB)…`);
  await u.waitFor(async () => (await u.text("#open-channel")).includes("Channel ready"), "channel ready", 300000);
  console.log(`[${u.name}] CHANNEL READY · account ${await u.text("#chan-addr")} · funding ${await u.text("#chan-fundtx")}`);
}

async function score(u) {
  await u.page.click("#score");
  // The page log PREPENDS: the auto-published bounty line lands on top of the
  // "scored" line — scan ALL log lines, not just the newest.
  const logLines = () => u.page.$$eval("#log li", (els) => els.map((el) => el.textContent ?? ""));
  await u.waitFor(async () => (await logLines()).some((l) => l.includes("scored")), "score committed", 120000);
  console.log(`[${u.name}] scored · ${(await logLines()).find((l) => l.includes("scored"))}`);
}

async function settleUser(u) {
  await u.page.click("#settle");
  await u.waitFor(async () => (await u.text("#settle")).includes("Settled"), "settle", 300000);
  console.log(`[${u.name}] SETTLED · ${await u.text("#chan-settle")}`);
}

stage("launch A + B (separate persistent profiles)");
console.log("navigating:", url);
const a = await launchUser("A", process.env.PROFILE_DIR_A ?? "D:/projects/ckb-controller/demo/.edge-profile-userA");
const b = await launchUser("B", process.env.PROFILE_DIR_B ?? "D:/projects/ckb-controller/demo/.edge-profile-userB");

try {
  stage("open channels to the hub (both users, in parallel)");
  await Promise.all([openChannel(a), openChannel(b)]);
  const [youA, youB] = [await a.text("#you"), await b.text("#you")];
  if (youA === youB) throw new Error("A and B show the SAME player hash — profiles are not isolated (check PROFILE_DIR_A/B)");

  stage("both score — the shared board must show both players");
  await score(a);
  await score(b); // B's score auto-publishes a bounty invoice via the relay
  await a.waitFor(async () => (await a.page.$$eval("#board tr", (r) => r.length)) >= 2, "both players on A's board", 30000);
  const rows = await a.page.$$eval("#board tr", (r) => r.length);
  console.log(`[A] board shows ${rows} players · seq ${await a.text("#seq")}`);

  stage("A pays the next bounty via the hub (trampoline) + the relay");
  await a.page.click("#pay-bounty");
  await a.waitFor(async () => (await a.text("#bounty-status")).includes("paid bounty"), "A's bounty payment", 180000);
  console.log(`[A] ${await a.text("#bounty-status")} · spent ${await a.text("#chan-spent")} CKB`);

  stage("assert the payment settled (operator match log)");
  const results = await (await fetch(`${OPERATOR}/results?n=50`)).json();
  const paid = (results.results ?? []).filter((e) => e.kind === "invoice_paid");
  if (paid.length === 0) throw new Error("no invoice_paid event in the operator results log");
  console.log(`[operator] invoice_paid events: ${paid.length} · last:`, JSON.stringify(paid[paid.length - 1]));

  stage("settle both channels");
  await Promise.all([settleUser(a), settleUser(b)]);

  stage("dump match log (GET /results)");
  const final = await (await fetch(`${OPERATOR}/results?n=50`)).json();
  for (const e of final.results ?? []) console.log("  ", JSON.stringify(e));

  stage("PASS — N-player match: shared state advanced AND a bounty settled over Fiber");
} finally {
  await Promise.allSettled([a.browser.close(), b.browser.close()]);
}
