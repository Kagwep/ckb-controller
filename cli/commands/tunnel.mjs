// tunnel — expose a testnet fnn over WSS for the browser demo's live mode.
// Thin wrapper over the proven runbook (../ckb-controller-cli/run-fnn-wss.sh),
// which starts fnn + a Cloudflare quick tunnel and prints the ready-to-paste
// ?live=1&peer=…&wss=… demo URL. Requires bash (Git Bash on Windows), the fnn
// binary, and cloudflared — see the script header for details.
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { loadCtx } from "../lib/config.mjs";

export async function run(_args, { configPath }) {
  const ctx = await loadCtx(configPath);
  const script = join(ctx.root, "..", "ckb-controller-cli", "run-fnn-wss.sh");
  if (!existsSync(script)) throw new Error(`runbook not found at ${script}`);
  console.log(`starting ${script} (Ctrl-C stops fnn + tunnel)…`);
  const child = spawn("bash", [script], { stdio: "inherit" });
  await new Promise((resolve, reject) => {
    child.on("exit", (code) => (code === 0 ? resolve() : reject(new Error(`tunnel exited with code ${code}`))));
    child.on("error", reject);
  });
}
