# Writing your game's rules

A game's rules live in **one file**: `game-rules/src/lib.rs`. That single crate
is compiled twice — into the on-chain type script that *enforces* every move,
and into the browser/operator stack that *precomputes* them. Because both sides
run the same code, the rule the chain enforces and the rule the client shows
cannot diverge. This is the one place Rust is required, and it is a small,
marked section.

## The template surface

Open `game-rules/src/lib.rs` and find the block between the `GAME RULE` /
`END GAME RULE` markers. The demo rule is a scoreboard: a move is "player scores
N points," capped per move. Three things change:

- **`PlayerEntry` / `GameState`** — what the board *is* (the state shape).
- **`Intent`** — what a *move* is.
- **`GameState::apply_intent`** — what a move *does*, plus your rule constants
  (the demo uses `MAX_POINTS_PER_MOVE = 1000`).

Keep the **nonce discipline** (`prev + 1`, first move = nonce 1). That is the
aggregator's anti-replay mechanism, not game logic — leave it in place for any
game.

## Free to change vs. wire format

| You may change freely | This is **wire format** — requires care |
|---|---|
| The state shape (`GameState`, `PlayerEntry`) | The byte *encodings* of state / intent / batch |
| What an intent means and what `apply_intent` does | The intent **signing message** |
| Rule constants (caps, limits) | The nonce framing |

Anything in the right-hand column is a shared contract between the type script,
the SDK, the wasm surface, and every JS client. If you change one, you must
update all of them **and**
[../internals/wire-formats.md](../internals/wire-formats.md) §10 in the same
change, plus the JS client code that hand-rolls those bytes. The left-hand
column is yours alone. When in doubt, keep the encodings and change only
`apply_intent` — that covers most games.

## Build both targets

```sh
node cli/bin.mjs build            # contracts (riscv) + wasm + pkg sync
node cli/bin.mjs build --docker   # same, contract toolchain in Docker
```

`build` compiles the type script and the wasm client and syncs the wasm package
into the demo and CLI. Test the rule the same way it runs in all three places:

```sh
cargo test -p controller-game-rules   # your rule's unit tests (fast, host)
cargo test -p tests                   # the full transaction set in CKB-VM
```

## Capacity planning

The game cell stores the board on-chain, and on CKB **bytes of data = CKB of
capacity**. The demo's board costs **36 bytes per player = 36 CKB per player**,
so a game cell genesis'd at 500 CKB holds roughly a dozen players plus overhead.

- Check headroom: `node cli/bin.mjs game show` prints the board and how many
  more players fit.
- When it fills up, new players are rejected with `InsufficientCellCapacity`.
  Enlarge it in place: `node cli/bin.mjs game grow <ckb> --send` (a no-op
  transition that only adds capacity). **Restart the operator afterward** — it
  tracks the game cell from its own transitions and needs to re-locate the new
  tip after any out-of-band change.
- Game cells **cannot be destroyed or split** — the type script allows only
  genesis (0-in, 1-out) and transition (1-in, 1-out).

## Shipping a rules change

A deployed game cell is bound to the code hash of the rules that created it.
Change the rules and the game script's code hash changes with it:

```sh
node cli/bin.mjs deploy --send
```

`deploy` detects that the freshly built binary's hash differs from the one in
the manifest and **automatically deploys the new script plus a fresh game
cell**. Old game cells continue under the old rules; the new rules receive new
cells. There is no migration and no in-place upgrade — old and new coexist.
