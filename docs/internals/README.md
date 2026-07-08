# Internals — maintainer documentation

For developers changing the controller itself (contracts, SDK, wasm, services,
CLI). Developers building games on top of it should read
[`../guide/`](../guide/README.md) instead.

| Page | Read it when |
|---|---|
| [architecture.md](./architecture.md) | you are new to the codebase, or unsure which crate a change belongs in |
| [wire-formats.md](./wire-formats.md) | you touch any byte layout (args, params, witnesses, Merkle, messages, game state) |
| [invariants.md](./invariants.md) | before changing behavior — the load-bearing rules and the pitfalls |
| [test-map.md](./test-map.md) | a test fails and you need to know which guarantee broke; or you added a guarantee and need to know what to test |
| [deployments.md](./deployments.md) | you are rebuilding binaries, verifying code hashes, or rolling the shared testnet deployment |

**The rule that keeps these accurate** (also in
[CONTRIBUTING.md](https://github.com/Kagwep/ckb-controller/blob/main/CONTRIBUTING.md)):
a PR that changes a wire format or an invariant must update the corresponding
page here *in the same PR*. These pages are versioned with the code they
describe; `plan.md` is the project log, and these are the stable reference.
