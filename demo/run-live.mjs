// Drive the LIVE browser demo end-to-end, headless: in-browser Fiber WASM node,
// session-signed external funding, N off-chain buys, cooperative settle.
// usage: node run-live.mjs <peerPubkey> <wssMultiaddr> [budgetCkb] [buys]
// Uses system Edge via puppeteer-core (no Chromium download).
import puppeteer from "puppeteer-core";

const [PEER, WSS] = [process.argv[2], process.argv[3]];
const BUDGET = process.argv[4] ?? "500";
const BUYS = Number(process.argv[5] ?? "5");
if (!PEER || !WSS) throw new Error("usage: node run-live.mjs <peerPubkey> <wssMultiaddr> [budgetCkb] [buys]");

const BASE = process.env.DEMO_URL ?? "http://localhost:5174";
const url = `${BASE}/?live=1&peer=${PEER}&wss=${encodeURIComponent(WSS)}`;
console.log("navigating:", url);

const browser = await puppeteer.launch({
  executablePath: "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  headless: "new",
  // Persistent profile: the in-browser Fiber node stores channel state in
  // IndexedDB — losing it orphans an open channel's funds until force-close.
  userDataDir: process.env.PROFILE_DIR ?? "D:/projects/ckb-controller/demo/.edge-profile",
  args: ["--no-first-run", "--disable-features=msEdgeIdentityFeatures"],
});
const page = await browser.newPage();
page.setDefaultTimeout(300000);
page.on("console", (m) => console.log(`[console:${m.type()}]`, m.text()));
page.on("pageerror", (e) => console.log("[pageerror]", e.message));

const text = (sel) => page.$eval(sel, (el) => el.textContent ?? "");
const latestLog = () => page.$eval("#log li", (el) => el.textContent ?? "").catch(() => "");
async function waitFor(pred, what, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (await pred()) return;
    if (Date.now() > deadline) throw new Error(`timeout waiting for ${what} (last log: ${await latestLog()})`);
    await new Promise((r) => setTimeout(r, 2000));
  }
}

try {
  await page.goto(url, { waitUntil: "networkidle2" });
  const iso = await page.evaluate(() => self.crossOriginIsolated);
  console.log("crossOriginIsolated:", iso);

  // budget, then connect (session approve + open channel via external funding)
  await page.$eval("#budget", (el, v) => ((el).value = v), BUDGET);
  await page.click("#connect");
  console.log(`clicked connect (budget ${BUDGET} CKB) — funding + ChannelReady can take ~2-3 min…`);

  await waitFor(
    async () => (await text("#connect")).includes("Channel ready"),
    "channel ready",
    300000,
  );
  const fundTx = await text("#fundtx");
  console.log(`CHANNEL READY · funding tx ${fundTx}`);

  for (let i = 1; i <= BUYS; i++) {
    await page.click("#buy1");
    await waitFor(
      async () => {
        const [troops, disabled] = await Promise.all([
          text("#troops"),
          page.$eval("#buy1", (el) => el.disabled),
        ]);
        return Number(troops) >= i && !disabled;
      },
      `buy ${i}`,
      60000,
    );
    console.log(`buy ${i}: troops=${await text("#troops")} spent=${await text("#spent")} CKB (off-chain)`);
  }

  await page.click("#settle");
  await waitFor(
    async () => (await text("#settle")).includes("Settled"),
    "settle",
    300000,
  );
  console.log(
    `SETTLED · local ${await text("#local")} · remote ${await text("#remote")} · settle tx ${await text("#settletx")}`,
  );
  const settleHref = await page
    .$eval("#settletx a", (a) => a.href)
    .catch(() => "(no link)");
  const fundHref = await page.$eval("#fundtx a", (a) => a.href).catch(() => "(no link)");
  console.log("funding explorer:", fundHref);
  console.log("settle explorer:", settleHref);
} finally {
  await browser.close();
}
