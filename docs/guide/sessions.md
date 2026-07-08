# The session model

A *session* is the core mechanism of the controller: the player approves once,
and the game receives a key that can act for them, but only within limits the
CKB chain enforces on every transaction. This page describes those limits as
user-facing concepts, the behavior when a game reaches them, and the
account-cell maintenance required to keep the system working. The byte-level
details are in [../internals/wire-formats.md](../internals/wire-formats.md).

## One approval, four constraints

When the owner authorizes a session, they create a session key carrying four
values the chain checks on each use.

### Budget — `spendCapCkb`

The spend cap is the **maximum net CKB that can leave the account cells in a
single transaction**. Formally it is *(capacity of the account inputs) −
(capacity returned to the account)*; if that exceeds the cap, the transaction
fails. This is the session's entire economic exposure, and it is also the budget
of any Fiber channel the session opens. A compromised session can drain at most
this amount per transaction, and never the owner's wallet.

### Expiry — `expiresAt`

Either a Unix timestamp in **seconds**, or `"never"`. After the timestamp, the
session key stops unlocking anything.

`"never"` remains safe because expiry is only one of four constraints. An
unexpiring session is still bounded by the spend cap and the policy — it cannot
administer the account or exceed the budget. (`"never"` also allows a session to
sign a counterparty-built transaction that carries no block-header reference,
which is what makes in-browser Fiber funding work.)

### Policy — `policiesRoot`

Where value is allowed to go. There are two modes:

- **`"wildcard"`** — the session may create any output. This is the simplest
  option; the budget and no-administer guards still apply.
- **Allow-list** — a Merkle root over a set of permitted destination scripts.
  The session may only create outputs whose **type-script or lock-script**
  matches an entry on the list (in addition to returning capacity to its own
  account). This is how a channel session is scoped to "may only fund the Fiber
  funding-lock, or settle back to the account" and nothing else.

### Guardian — `guardian`

An optional co-signer. `null` means none. If set, **every** session move
requires a second signature from the guardian key in addition to the session
key — a two-of-two arrangement for higher-sensitivity deployments.

## Revocation

How a session is ended depends on which of the two trust models created it:

- **Registered** (the default, and what `init` scaffolds): the session's limits
  are stored in the account cell itself. The owner ends it with one owner-mode
  transaction that re-creates the account under new parameters (a new session
  key, or none). Because the parameters are part of the address, **the address
  changes** — plan for this, or use a real `expiresAt` instead of `"never"` if
  you want sessions to expire on their own.
- **Carried**: the session's limits ride in every transaction, authorized by an
  owner signature bound to a monotonic **revocation epoch** stored in the
  account cell's data. The owner revokes by incrementing the epoch (one
  owner-mode transaction; **the address does not change**), and every session
  authorized under the old epoch immediately stops working.

In both models, a session key can never un-revoke itself: the lock forbids a
session from modifying the account's owner, parameters, or data.

## Behavior at each limit

A game will reach these limits; the on-chain failure for each is listed below.
Reads and dry-runs never spend, so these are detected before broadcasting.

| Situation | On-chain result |
|---|---|
| Move would send more than the budget | `SpendCapExceeded` — the tx is rejected |
| Session has expired | `SessionExpired` |
| Output goes somewhere the policy does not allow | `PolicyNotAllowed` (or `PolicyProofMissing` / `PolicyProofMalformed` if the proof is absent or malformed) |
| Guardian signature missing when one is required | `WitnessLenError` (absent) or `AuthError` (wrong key) |
| Session attempts to rewrite the account's owner, args, or data | `SessionCannotAdminister` |
| Two account cells spent in one tx | `MultipleInputs` |

The SDK surfaces these as thrown errors; nothing partially succeeds.

## The one-account-cell rule and maintenance

The lock allows **exactly one account cell as input per transaction**. This
keeps the accounting unambiguous, but it means the account must remain a *single
live cell*. Two routine operations affect it:

- **`account grow <ckb> --send`** — top up the account **in place**. It
  session-signs the account cell plus one funding input into one larger account
  cell. The spend cap is unaffected because capacity flows *in*.
- **`account drain --send`** — send the smallest account cell back to your plain
  key. Run this after a channel **settle**, which leaves two cells (the funding
  change plus the returned balance); draining merges them back to one. The
  command refuses up front if the amount would exceed the spend cap, because a
  session-signed drain over the cap would fail on-chain.

Check the state at any time:

```sh
node cli/bin.mjs account show
```

It reports whether you have one cell (correct) or need to drain. `status`
reports the same and warns if the count is not 1.
