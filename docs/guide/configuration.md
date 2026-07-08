# Configuration

The tool is driven by one pair of files, located by upward search from your
working directory (override with `--config <path>` or the `CONTROLLER_CONFIG`
environment variable):

- **`controller.config.json`** — your settings: network, keys, session policy,
  operator and Fiber endpoints.
- **`.controller/manifest.json`** — facts about deployed on-chain artifacts,
  keyed by network. **The tool writes this; you do not.**

`init` scaffolds both. This page is the reference for every field.

---

## `controller.config.json`

```json
{
  "network": "testnet",
  "keyFile": "./testnet-key.txt",
  "gameId": "0x0000…0000",
  "session": {
    "ownerPrivkey": "0x…",
    "sessionPrivkey": "0x…",
    "spendCapCkb": 1000,
    "expiresAt": "never",
    "policiesRoot": "wildcard",
    "guardian": null
  },
  "operator": { "listen": "127.0.0.1:9944", "chain": "http", "feeShannons": 100000 },
  "fiber": { "rpc": "http://127.0.0.1:8227" },
  "networks": {
    "testnet": { "rpc": "https://testnet.ckb.dev/rpc", "explorerTx": "https://testnet.explorer.nervos.org/transaction/" },
    "devnet":  { "rpc": "http://127.0.0.1:8114", "explorerTx": "", "ckbDir": "…/ckb_v0.207.0" }
  }
}
```

### Top level

| Field | Meaning |
|---|---|
| `network` | Which entry of `networks` (and which manifest section) is active: `testnet` or `devnet`. |
| `keyFile` | Path to the deploy/operator private key (raw hex, one line). Relative paths resolve against the config's directory, not your cwd. Override per machine with the `KEYFILE` environment variable. |
| `gameId` | 32-byte hex id of your game cell. Browser clients sign intents bound to this exact id, so client and cell must agree. `init` generates a fresh one. |

### `session` — the session policy

These fields *are* the on-chain limits the lock enforces. Their meaning as
user-facing concepts is in [sessions.md](./sessions.md); this table gives the
encoding.

| Field | Meaning |
|---|---|
| `ownerPrivkey` | The owner key. Only owner-mode transactions can change the account's setup or revoke sessions. |
| `sessionPrivkey` | The session key the game signs with day to day. |
| `spendCapCkb` | Maximum **net CKB outflow from the account per transaction**. This is also the channel budget. |
| `expiresAt` | `"never"` (a safe value — the cap and policy still bind) or a Unix timestamp in **seconds**, after which the session key stops working. |
| `policiesRoot` | `"wildcard"` (session may create any output) or a 32-byte Merkle root over an allow-list of destination scripts. |
| `guardian` | `null` for no guardian, or a pubkey-hash hex, in which case a guardian co-signature is required on every session move. |

### `operator`

| Field | Meaning |
|---|---|
| `listen` | Address the game operator serves on (default `127.0.0.1:9944`). Clients and `status` look here. |
| `chain` | Chain transport for the operator (`http`). |
| `feeShannons` | Fee the operator attaches per transition (100000 = 0.001 CKB). |

### `fiber`

| Field | Meaning |
|---|---|
| `rpc` | RPC URL of a native Fiber node, used by non-browser Fiber tooling. The browser demo runs its Fiber node in-page instead (see [going-live.md](./going-live.md)). |

### `networks.<name>`

| Field | Meaning |
|---|---|
| `rpc` | CKB JSON-RPC URL for that network. |
| `explorerTx` | Base URL for transaction links (`status` and the CLI append the tx hash). Empty prints the bare hash. |
| `ckbDir` | **devnet only** — path to a `ckb_v0.2xx` release directory. `dev` needs it to boot the local chain (or set `CKBDIR`). |

---

## `.controller/manifest.json`

Deployed artifacts per network. Each entry is one on-chain code cell or dep:

```json
"game": {
  "codeHash": "0x81fa…",          // data hash of the built binary
  "hashType": "data2",
  "dep": { "txHash": "0x2d3c…", "index": "0x0" },
  "depType": "code"
}
```

The tracked artifacts are `lock`, `auth`, `secp256k1Sighash`, and `game`.

**Written by:** `deploy` (and `dev`), which update it as each artifact lands.
**When to hand-edit it: never**, with one exception — *adopting someone else's
deployment* (paste their code-cell out-points to point your project at cells
they deployed). `init` pre-seeds the `testnet` section with the shared public
code cells so a new game pays only for its own game cell and account.

If a code cell you rely on is reported "NOT live" by `status`, do not edit the
manifest by hand — re-run `deploy --send` (with the binaries built) to publish
your own copy.

---

## Environment overrides

Each of these overrides a single config value everywhere the tool reads config,
which is convenient for CI or one-off runs without editing the file:

| Env var | Overrides |
|---|---|
| `NETWORK` | `network` |
| `RPC` | the active network's `rpc` |
| `KEYFILE` | `keyFile` |
| `GAME_ID` | `gameId` |
| `CONTROLLER_CONFIG` | the config file location (not a field) |
| `CKBDIR` | `networks.devnet.ckbDir` (for `dev`) |

---

## A note on keys

The keys `init` writes — and the fixed keys in the repository's demo config —
are **demo-grade**: plain hex private keys stored in a file. They are acceptable
for a dev chain or public **testnet**, and they let the tool reconstruct your
account address deterministically. They are **not** a key-management solution
for real value. Before anything touches mainnet value, move to proper key
management (hardware, KMS, or per-user generated keys). The scaffolded config
states this in its own header comment.
