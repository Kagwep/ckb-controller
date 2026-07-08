# Game developer guide

The controller lets a game act on a player's behalf without prompting for a
wallet signature on every move. The player approves a *session* once; from then
on the game signs moves and payments with a session key whose permissions are
enforced by the CKB chain itself: a spend cap, an optional expiry, an allow-list
of permitted destinations, and revocation. On-chain **state** (scores, board
moves) is committed through a game-cell aggregator; high-frequency **value**
(in-game purchases, tips) is settled off-chain over a
[Fiber](https://github.com/nervosnetwork/fiber) payment channel whose budget is
the same on-chain cap. The guarantee this provides: a compromised game can lose
at most the session's budget, never the player's wallet.

Building a game requires Node.js and browser development. Rust is required only
to change the game's rules, which live in a single file (see
[your-game.md](./your-game.md)).

## Documentation map

| Goal | Page |
|---|---|
| Get a game running in ~15 minutes (local or testnet) | [quickstart.md](./quickstart.md) |
| Reference for every config field and environment override | [configuration.md](./configuration.md) |
| Understand what a budget, expiry, policy, and revocation mean, and the behavior at each limit | [sessions.md](./sessions.md) |
| Change what a move is by writing custom game rules | [your-game.md](./your-game.md) |
| Move from a local chain to public testnet (operator, live channels, known issues) | [going-live.md](./going-live.md) |
| Understand what is enforced where, and the bound on a compromise | [trust.md](./trust.md) |

The runtime you code against is `@ckb-controller/sdk`
([sdk-js/README.md](https://github.com/Kagwep/ckb-controller/blob/main/sdk-js/README.md));
the command-line interface is the CLI
([cli/README.md](https://github.com/Kagwep/ckb-controller/blob/main/cli/README.md)).
Byte-level formats and other maintainer material are in
[docs/internals/](../internals/); they are not required to ship a game.
