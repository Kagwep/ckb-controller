# Deployments

For rebuilding binaries, verifying code hashes, and rolling the shared testnet
deployment. The governing fact throughout: on CKB a script's **code hash is the
data hash of its binary**, referenced `hash_type = data2` (see
[invariants.md §1](./invariants.md)). Two byte-identical binaries have the same
code hash and are interchangeable as deps; one changed byte is a new script.

## Building the binaries

| What | Command | Output |
|---|---|---|
| the two contracts (riscv64) | [`./build.sh`](https://github.com/Kagwep/ckb-controller/blob/main/build.sh) (or `make build`) | `build/release/controller-session-lock`, `build/release/controller-game-cell` |
| the wasm client + JS pkg | [`./scripts/build-wasm.sh`](https://github.com/Kagwep/ckb-controller/blob/main/scripts/build-wasm.sh) | `wasm/pkg/{controller.js,.d.ts,_bg.wasm}` |
| **both**, and sync the pkg | `node cli/bin.mjs build` | contracts + wasm, then `wasm/pkg` copied into `demo/pkg` and `cli/pkg` |

`build.sh` cross-compiles with a clang 16+ (`scripts/find_clang` locates it) and
the `+zb*` extensions; `build-wasm.sh` needs a `wasm-bindgen-cli` matching the
lib version — both toolchain pins are explained in
[invariants.md §11](./invariants.md). `node cli/bin.mjs build` is the command
behind the game template: edit `game-rules/src/lib.rs`, `build`, then
`deploy --send`.

A **`docker/build.Dockerfile`** exists for a reproducible contract build
(`build --docker`), but it is **unverified** — there is no Docker on the machine
these were developed on, so only the native path is tested
([plan.md](https://github.com/Kagwep/ckb-controller/blob/main/plan.md),
Direction 1D). Do not treat the Docker image as known-good without running it.

## Reproducibility

The release profile in
[`Cargo.toml`](https://github.com/Kagwep/ckb-controller/blob/main/Cargo.toml) is
deterministic (`strip = true`, `codegen-units = 1`, `overflow-checks = true`), so
an unchanged crate rebuilds to a byte-identical binary and therefore the
identical code hash. This was observed directly on **2026-07-08**: after the
`game-rules` refactor, the untouched lock **rebuilt to the identical hash**
(`deploy` skipped it), while the refactored game type script produced a **new**
hash — local `0xc8c98a78…` vs the deployed `0x81fa44f5…` — and `deploy` flagged
it as new code to ship
([plan.md](https://github.com/Kagwep/ckb-controller/blob/main/plan.md),
Direction 1D).

`deploy` performs that check: it computes `ccc.hashCkb(binary)` (the CKB blake2b
data hash) and compares it to the manifest's `codeHash`; a mismatch means the
live old deployment does not satisfy this artifact, so it redeploys
([`cli/commands/deploy.mjs`](https://github.com/Kagwep/ckb-controller/blob/main/cli/commands/deploy.mjs)).
For the vendored ckb-auth, `deps/checksums.txt` records the binary's checksum —
use it to confirm a third-party testnet auth deploy is byte-identical before
reusing it (which saves the ~151k CKB of redeploying auth).

## The shared testnet deployment — two records to keep in sync

The public testnet code cells are recorded in **two** places that must agree:

- [`.controller/manifest.json`](https://github.com/Kagwep/ckb-controller/blob/main/.controller/manifest.json)
  — what the repo (drivers, browser, operator, CLI) reads for the active
  network.
- [`cli/lib/known-deployments.mjs`](https://github.com/Kagwep/ckb-controller/blob/main/cli/lib/known-deployments.mjs)
  — shipped with the CLI so `ckb-controller init` pre-seeds a **new** project's
  manifest with the shared cells, so a fresh game costs ~1k CKB (its own game +
  account cell) instead of a ~314k CKB redeploy.

**Constraint:** these are hand-maintained copies of the same out-points. If you
roll a new testnet deployment, update **both** — the manifest so this repo uses
it, and `known-deployments.mjs` so freshly `init`'d projects inherit it. Nothing
enforces their equality today.

Current testnet out-points (copied from the manifest; verify against it before
relying on them):

| Artifact | Code hash (data2) | Dep out-point (`depType`) |
|---|---|---|
| lock | `0x9d3ce3e29c65467fdff3ece23883e54a5fb03e677d9da80879691a9823034a9c` | `0x2d754da027c1c90dad7169c55cdef666644258c1e5bf02f49b112bf525fc9b93:0x0` (code) |
| ckb-auth | — (vendored binary) | `0x539e202c058680b1945352800ad8d6edaaf2ec2034d6b2d575aad423bf1a401c:0x0` (code) |
| game type script | `0x81fa44f5eb7209d4ef5b2c5b10679eac1ff8d76b18ee8006af48b2c76e330d6c` | `0x2d3cda90d8b348ab28a6f55d87e11b580eec00419b6d67318a3ba92b52bca17b:0x0` (code) |
| secp256k1_blake160 sighash | — (genesis) | `0xf8de3bb47d055cdf460d93a2a6e1b05f7432f9777c8c474abf4eec1d4aee5d37:0x0` (depGroup) |

The manifest also carries a `devnet` block, populated per-run by
`ckb-controller dev`.

## Costs

On-chain storage is **1 CKB per byte**, so the code cells are the expense. Sizes
from `build/release/` + `deps/`:

| Binary | Size | Code-cell capacity (≈ size + ~200 CKB cell overhead) |
|---|---|---|
| `controller-session-lock` | 94,776 bytes | ~95k CKB |
| `deps/auth` (ckb-auth) | 150,904 bytes | ~151k CKB |
| `controller-game-cell` | 68,936 bytes | ~69k CKB |

(The CLI reserves `code.length + 200` CKB per code cell —
[`deploy.mjs`](https://github.com/Kagwep/ckb-controller/blob/main/cli/commands/deploy.mjs).
The `run-testnet.sh` header quotes slightly different round figures — lock
~94,876, auth ~151,004 — treat the `ls` byte sizes above as ground truth; the CKB
figure is bytes plus a small cell overhead.) A from-scratch testnet deploy of
lock + auth + account is ~246k CKB, which fits inside a single ~300k faucet
claim; reusing an existing auth deploy saves ~151k. **The capacity is locked, not
burned** — it is reclaimable later by the deployer key by consuming the cells.

## Rolling a new version

1. **Build** the changed binary: `node cli/bin.mjs build` (or `./build.sh`).
2. **Deploy** the missing/changed pieces: `ckb-controller deploy --send`. It is
   idempotent — unchanged code cells are skipped by the hash comparison above;
   only a changed binary (new data hash) is redeployed, and only what is missing
   is created (code cell → game-cell genesis → account cell). Dry-run first
   (default, no `--send`) to see the plan.
3. **Understand the binding.** A game cell's type script pins a specific code
   hash, so **new game cells bind to the new code**; **existing game cells keep
   running under the old code** they were created with. There is no in-place
   upgrade of a live cell — a rolled script is a parallel deployment, and the old
   one stays canonical for games already using it (as the 2026-07-08 game-rules
   refactor did — behaviorally identical, so the demo game was left on the old
   code).
4. **Propagate the out-points** to both sync targets above: the manifest (done
   automatically by `deploy --send` for this repo) **and**
   `cli/lib/known-deployments.mjs` (manual — for newly `init`'d projects).
5. **Verify**: `ckb-controller status` (chain tip, each code cell live?, account
   single-cell?, game seq, operator health), then a smoke intent — post a
   session-signed move and confirm the deployed type script commits the
   transition.

## The deployer key holds the code cells — protect them

A deployed code cell is a **live cell owned by the deployer key**, sitting
alongside that key's plain (spendable) cells. If a deploy/genesis/account tx were
to select a code cell as a fee input, it would consume — and lose — the
deployment.

The guard is **plain-cell-only input selection**: `selectPlainCells` skips any
cell with data or a type script
(`cell.outputData !== "0x" || cell.cellOutput.type`), and `assertOnlyInputs`
aborts if the final input set contains anything unexpected — both in
[`cli/lib/config.mjs`](https://github.com/Kagwep/ckb-controller/blob/main/cli/lib/config.mjs),
used by every state-changing CLI command. This is the `fund.mjs` lesson: never
let generic fee-funding touch a code cell. When adding a new tx-building path,
reuse `selectPlainCells` + `assertOnlyInputs` rather than writing new input
selection.
