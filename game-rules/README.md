# game-rules — your game, compiled twice

This crate is the **game template**: the single place where the game's state
model and transition rule live. It is compiled into BOTH:

- the **on-chain type script** (`contracts/controller-game-cell`, riscv64) —
  the rule the chain *enforces*: every transition is re-derived and every
  intent signature checked; the operator can sequence but never cheat;
- the **off-chain stack** (`sdk/` → `wasm/` → the browser and the operator) —
  the rule the client *precomputes* and displays.

One crate, two targets: the enforced rule and the client's rule **cannot
drift**. (Before this crate existed, the rules were duplicated and only sanity
tests kept them honest.)

## Making your game

1. Edit the marked `GAME RULE` section in [`src/lib.rs`](./src/lib.rs):
   - `PlayerEntry` / `GameState` — what the board is,
   - `Intent` — what a move is,
   - `GameState::apply_intent` — what a move *does* (+ your rule constants).

   Keep the nonce discipline (`prev + 1`, first move = 1) — it is the
   aggregator's anti-replay. If you change the *encodings* (state layout,
   intent layout, the signing message), you are changing the wire format:
   update `docs/internals/wire-formats.md` §10 and the JS clients in the same
   change.

2. Rebuild both artifacts:

   ```sh
   node cli/bin.mjs build            # contracts (riscv) + wasm + pkg syncs
   node cli/bin.mjs build --docker   # same, contract toolchain in docker
   ```

3. Test — the same rules run in three harnesses:

   ```sh
   cargo test -p controller-game-rules   # your rule's unit tests (host)
   cargo test -p tests                   # the full tx set in CKB-VM, real binary
   ```

4. Ship it:

   ```sh
   node cli/bin.mjs deploy --send    # detects the changed code hash, deploys the
                                     # new script + a fresh game cell
   ```

   A deployed game cell is bound to the code hash that created it: old cells
   keep playing by the old rules; your new rules get a new script + new cells.
   (`deploy` sees the local binary's hash differs from the manifest and does
   the right thing.)

## Operational notes

- Each player entry costs its encoded size in cell capacity (36 bytes = 36 CKB
  for the demo scoreboard). A full game cell rejects new players with
  `InsufficientCellCapacity` — grow it in place with
  `node cli/bin.mjs game grow <ckb> --send` (an empty-batch no-op transition
  that only adds capacity).
- The operator (`paymaster-service/src/bin/game-operator.rs`) is game-agnostic:
  it batches intents, finalizes, broadcasts. It tracks the game cell from its
  own flushes — after an out-of-band transition (like `game grow`), restart it
  so it re-locates the tip.
