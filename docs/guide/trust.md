# Trust and safety

The design answers a single question: if something in a game is compromised, how
much can be lost? The answer is "at most the session's budget, never the
wallet" — and this is not a policy claim, it is enforced by the CKB chain. This
page describes **what is enforced where**, and **what a compromise yields**.

## Three enforcement tiers

### Tier 1 — the chain (cannot be bypassed by anyone)

Two on-chain scripts run in CKB-VM on every relevant transaction. Nothing — not
the operator, not the SDK, not the maintainers — can override them.

**The session lock** checks, on every session move:

- **budget** — net CKB out of the account cannot exceed the spend cap;
- **expiry** — an expired session key unlocks nothing;
- **policy** — outputs must match the session's allow-list (by type or lock
  script), unless the policy is wildcard;
- **no-administer** — a session can never rewrite the account's owner, args, or
  data (so it cannot lift its own limits or cancel revocation);
- **one input** — exactly one account cell per transaction.

**The game type script** checks, on every transition:

- **every intent signature** — verified via ckb-auth, per move;
- **the exact transition** — the output board must equal the rule re-applied to
  the input board; the operator's arithmetic is re-derived, not trusted;
- **no genesis seeding** — a new game cell must start empty;
- **no destruction** — the cell can only genesis or transition, never be
  destroyed or split.

### Tier 2 — the operator (liveness only)

The operator batches intents and broadcasts transitions. That is the full extent
of its power. It can **delay, censor, or reorder** moves — which is visible and
recoverable. It can **never** forge a move (it holds no player's session key),
tamper with a result (the type script re-derives the transition), or spend an
account (it is not the account's session key). A batch the node rejects — for
example, a forged signature — is simply removed from its queue.

### Tier 3 — the SDK / client (convenience only)

The SDK assembles witnesses, computes signing messages, manages nonces, and
handles Fiber's details. If it were buggy or hostile, it still could not produce
anything the chain accepts beyond what the session key is allowed to sign. It is
a convenience layer over Tier 1, not a security boundary.

## What a compromise yields

| Compromised component | Worst case | Why it is bounded |
|---|---|---|
| **Game or session key** | At most the session **budget** (spend cap), per tx — never the owner's wallet | The lock caps net outflow, enforces the policy, and forbids administering the account. The owner key is separate. |
| **Operator** | Stalls or censors the game; visible and recoverable | It cannot forge a move (no session keys) or tamper with a result (the type script re-derives every transition). |
| **Relayer / paymaster** | Cannot exceed what the session already signed | The relayer assembles the tx and balances fees *before* the client session-signs (assemble-then-sign), so the session signature covers the final transaction. It can pay for gas, not change the transaction. |

The consistent property: the keys that hold real value (the owner key, the
wallet) never travel through the game, the operator, or the browser Fiber node.
The session key that does travel is bounded by the chain.

## Verifying it independently

None of this needs to be taken on trust.

- **Watch the chain.** Every transition and settle is a real transaction. The
  CLI and `status` print explorer links (from `networks.<net>.explorerTx`); the
  SDK exposes `controller.explorerTx(hash)`. Open them and read the cells.
- **Check state at any time.** `node cli/bin.mjs status` shows the live game
  cell's sequence and player count, your account cell count, and operator
  health, independent of any client.
- **Run the enforcement suite.** The entire on-chain rule set — every lock check
  and every game-cell transition, each with a passing case and a rejection
  case — runs in CKB-VM against the real binaries:

  ```sh
  cargo test -p tests
  ```

For the byte-exact contract behind all of this, see
[../internals/wire-formats.md](../internals/wire-formats.md).
