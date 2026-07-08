// Computes AUTH_CODE_HASH from the ckb-auth binary so the lock can delegate
// signature verification to it via spawn_cell (same pattern as fiber-scripts).
//
// During scaffolding the `deps/auth` binary may not be present yet; in that case
// we emit a zero hash and a warning instead of failing the build, so the skeleton
// stays compilable. Drop the real ckb-auth binary at `deps/auth` before deploying.
use ckb_gen_types::{packed::CellOutput, prelude::*};
use std::env;
use std::fs::{read, File};
use std::io::{BufWriter, Write};
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../deps/auth");

    let code_hash: [u8; 32] = match read("../../deps/auth") {
        Ok(auth_binary) => {
            let hash = CellOutput::calc_data_hash(&auth_binary);
            hash.as_slice().try_into().expect("32-byte data hash")
        }
        Err(_) => {
            println!(
                "cargo:warning=deps/auth not found; AUTH_CODE_HASH set to zero. \
                 Place the ckb-auth binary at deps/auth before deploying."
            );
            [0u8; 32]
        }
    };

    let out_path = Path::new(&env::var("OUT_DIR").unwrap()).join("auth_code_hash.rs");
    let mut out_file = BufWriter::new(File::create(out_path).expect("create auth_code_hash.rs"));
    writeln!(
        &mut out_file,
        "pub const AUTH_CODE_HASH: [u8; 32] = {:#02X?};",
        code_hash
    )
    .expect("write auth_code_hash.rs");
}
