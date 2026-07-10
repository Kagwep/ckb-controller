// Print ONE profile's per-user controller account address (Phase 2 runbook §3):
// loads ?multi=1 (mock — no peer needed) in the given persistent profile so the
// per-user keypair is minted/persisted, and reads the full address from the
// footer's title attribute. Run once per profile BEFORE fund-user.mjs.
// usage: node get-user-addr.mjs <profileDir> [demoUrl]
import puppeteer from "puppeteer-core";

const PROFILE = process.argv[2];
const BASE = process.argv[3] ?? process.env.DEMO_URL ?? "http://localhost:5173";
const EDGE = process.env.EDGE_PATH ?? "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
if (!PROFILE) throw new Error("usage: node get-user-addr.mjs <profileDir> [demoUrl]");

const browser = await puppeteer.launch({
  executablePath: EDGE,
  headless: "new",
  userDataDir: PROFILE,
  args: ["--no-first-run", "--disable-features=msEdgeIdentityFeatures"],
});
try {
  const page = await browser.newPage();
  page.setDefaultTimeout(60000);
  await page.goto(`${BASE}/?multi=1`, { waitUntil: "networkidle2" });
  await page.waitForSelector("#user-addr:not(.hidden)");
  const addr = await page.$eval("#user-addr", (el) => el.title);
  if (!addr.startsWith("ckt1")) throw new Error(`unexpected address: ${addr}`);
  console.log(addr);
} finally {
  await browser.close();
}
