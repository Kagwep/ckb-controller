// Integration tests run the compiled lock against mock transactions using
// ckb-testtool (the same harness fiber-scripts uses).
//
// Requires the lock binary at ../build/release/controller-session-lock
// (run `./build.sh` or `make build`) and the ckb-auth binary at ../deps/auth.

#[cfg(test)]
mod tests;

#[cfg(test)]
mod game_tests;

#[cfg(test)]
mod game_operator_sanity;

#[cfg(test)]
mod sdk_sanity;

#[cfg(test)]
mod paymaster_sanity;

#[cfg(test)]
mod service_sanity;
