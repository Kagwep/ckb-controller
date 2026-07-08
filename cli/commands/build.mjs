// build — compile the game (and lock) from source and sync every consumer:
//
//   1. contracts:  ./build.sh        -> build/release/{controller-session-lock,
//                                       controller-game-cell}   (riscv64, VM2)
//   2. wasm:       scripts/build-wasm.sh -> wasm/pkg            (browser client)
//   3. sync:       wasm/pkg -> demo/pkg + cli/pkg
//
// This is the command behind the game template: edit game-rules/src/lib.rs,
// `ckb-controller build`, and both the on-chain type script and the browser
// client carry the SAME rules (one crate, two targets — no drift possible).
//
// Native path needs the repo toolchain (rustup riscv64 target, a clang for
// ckb-std's C stub, wasm-bindgen-cli — see the root README). `--docker` runs
// the contract build inside the toolchain image instead (docker/build.Dockerfile),
// so no local riscv/clang setup is required.
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { cp, mkdir } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { loadCtx } from "../lib/config.mjs";

function sh(cmd, args, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { cwd, stdio: "inherit", shell: false });
    child.on("exit", (code) => (code === 0 ? resolve() : reject(new Error(`${cmd} ${args.join(" ")} exited ${code}`))));
    child.on("error", reject);
  });
}

export async function run(args, { configPath }) {
  const ctx = await loadCtx(configPath);
  const docker = args.includes("--docker");
  const skipWasm = args.includes("--no-wasm");

  if (!existsSync(join(ctx.root, "build.sh"))) {
    throw new Error(`no build.sh at ${ctx.root} — \`build\` needs a ckb-controller repo checkout (standalone projects use the prebuilt binaries + shared deployments)`);
  }

  // 1. contracts
  if (docker) {
    console.log("building contracts in docker (ckb-controller-toolchain)…");
    await sh("docker", [
      "run", "--rm",
      "-v", `${ctx.root}:/work`,
      "-w", "/work",
      "ckb-controller-toolchain",
      "bash", "./build.sh",
    ], ctx.root);
  } else {
    console.log("building contracts (native toolchain)…");
    await sh("bash", ["./build.sh"], ctx.root);
  }

  // 2. wasm client
  if (!skipWasm) {
    console.log("building wasm client…");
    await sh("bash", ["scripts/build-wasm.sh"], ctx.root);
  }

  // 3. sync the wasm pkg to every consumer
  const src = join(ctx.root, "wasm", "pkg");
  const cliPkg = fileURLToPath(new URL("../pkg", import.meta.url));
  for (const dest of [join(ctx.root, "demo", "pkg"), cliPkg]) {
    if (!existsSync(src)) break;
    await mkdir(dest, { recursive: true });
    await cp(src, dest, { recursive: true });
    console.log(`synced wasm/pkg -> ${dest}`);
  }

  console.log("\nbuild complete. Changed the rules (game-rules/)? The game script's");
  console.log("code hash changed with it — `ckb-controller deploy --send` puts the");
  console.log("new script + a fresh game cell on-chain (old game cells keep playing");
  console.log("by the old rules; a game id is bound to the code hash that made it).");
}
