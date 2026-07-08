# Contributing

## The one rule that keeps this repo safe to change

The lock, the game type script, the Rust SDK, the wasm surface, and the JS
clients all encode the **same bytes**. If two of them disagree, funds lock up
or forged data validates — there is no partial failure. So:

> **A PR that changes a wire format or an invariant MUST, in the same PR:**
> 1. change every implementation of that format
>    (see the matrix in [docs/internals/wire-formats.md](./docs/internals/wire-formats.md) §11),
> 2. update the corresponding page under [docs/internals/](./docs/internals/README.md),
> 3. update or add the tests that pin the behavior
>    (find them via [docs/internals/test-map.md](./docs/internals/test-map.md)).

## Checklist by change type

- **Byte layout** (args, session params, witness, proof region, signing
  message, game state/intent/batch) → wire-formats.md + all implementations +
  the relevant `*_sanity` in-VM test. If it's a *game* encoding, that includes
  the JS clients and `docs/guide/your-game.md`.
- **Game rules** (the marked section in `game-rules/src/lib.rs`) → rebuild both
  targets (`node cli/bin.mjs build`), run `cargo test -p tests`; a rules change
  means a NEW code hash → a new deployment (old cells keep old rules).
- **Lock/type-script behavior** → docs/internals/invariants.md + an in-VM test
  with BOTH a pass and a reject case (the suite's convention).
- **Error codes** → they are observable on-chain interface; wire-formats.md
  §9/§10 tables must match the enums exactly.
- **Config/manifest schema** → `cli/lib/config.mjs`, `demo/controller-config.mjs`,
  `demo/src/config.ts`, `sdk-js/src/types.ts`, the operator's config loading,
  and `docs/guide/configuration.md`.
- **Shared testnet deployment** → follow docs/internals/deployments.md; keep
  `.controller/manifest.json` and `cli/lib/known-deployments.mjs` in sync.

## Verifying

```sh
./build.sh                       # contracts (riscv64; needs clang 16+)
cargo test -p tests              # the full tx suite in CKB-VM (needs host gcc)
cargo test --workspace           # host-unit suites
node cli/bin.mjs status          # against the configured network
```

`plan.md` is the running project log (append, don't rewrite history);
`docs/` is the stable reference. When they disagree, fix `docs/`.
