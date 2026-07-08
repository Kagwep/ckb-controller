# controller-wasm

Browser/JS client for the CKB controller session lock — a `wasm-bindgen` surface
over `controller-sdk`. It's the **L1 authorization** layer in the browser: account
address, session args/params, the carried-model auth message, the policy Merkle
tree, mode-tagged witnesses, the cell_deps-cleared signing message, and the
`ChannelSession` open→pay→close loop.

It is **not** a Fiber node. Off-chain payment streaming is delegated to Fiber's own
JS light client (`@nervosnetwork/fiber-js`) on the JS side, which implements the
L2 rail; this crate does the L1 authorization. (cf. `../fiber-charge-sim` —
"use Fiber, don't duplicate".)

## Build

```sh
../scripts/build-wasm.sh        # -> wasm/pkg/{controller.js,.d.ts,_bg.wasm}
```

Needs a `wasm-bindgen-cli` matching the `wasm-bindgen` version in `Cargo.toml`
(see that file's note — build the CLI with a newer toolchain; the crate itself
stays on the workspace's rustc 1.85.1).

## Use

```ts
import init, {
  script, registered_args, session_params, controller_address,
  tx_message, session_witness_registered, ChannelSession,
  no_expiry, wildcard_root, spend_cap_unlimited,
} from "./pkg/controller.js";

await init();

// derive the controller account address (data2 = 0x04 hash type, testnet)
const args = registered_args(ownerHash, session_params(
  sessionHash, no_expiry(), wildcard_root(), spend_cap_unlimited(), guardianHash,
));
const addr = controller_address(lockCodeHash, 0x04, args, true);

// session-funded payment channel, in the browser
const ch = new ChannelSession(accountLockScript, `${txHash}:0`, 100_000_000_000n,
                              fundingLockScript, headerHash);
const open = ch.open("peer-node", 50_000_000_000n);
const sig = signWithSessionKey(open.message);          // your wallet, recoverable secp256k1
const witness = session_witness_registered(sig, "", channel_proof_region());
// ...attach `witness`, broadcast `open.tx`, then:
for (let i = 0; i < 5; i++) ch.pay(1_000_000_000n);    // off-chain, no L1
const close = ch.close();                               // close.settle.tx -> sign + broadcast
```

Every function takes/returns `0x…` hex (or decimal strings for `u128`). The caller
signs the `message` fields (65-byte recoverable secp256k1) and feeds the signatures
into the witness builders — the crate stays agnostic to the JS wallet in use.
