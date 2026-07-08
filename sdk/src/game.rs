//! The game rules — re-exported from the shared `controller-game-rules` crate,
//! which is compiled into BOTH this off-chain SDK (→ wasm → browser) and the
//! on-chain type script (`contracts/controller-game-cell`). One source of
//! truth; the rules cannot drift between chain and client.
//!
//! To change the game, edit `game-rules/src/lib.rs` (the marked GAME RULE
//! section) and rebuild both artifacts (`ckb-controller build`).
pub use controller_game_rules::*;
