//! Header generation for the C API.
//!
//! When the `c_api` feature is enabled, `include/linkrs.h` is regenerated from
//! crates/linkrs-api with cbindgen so the committed header can never drift
//! away from the Rust sources. Without the feature nothing runs and regular
//! builds stay unaffected.

use std::process::Command;

const HEADER: &str = "include/linkrs.h";
const CONFIG: &str = "cbindgen.toml";
const C_API_CRATE: &str = "crates/linkrs-api";

fn main() {
    // The header mirrors these inputs; anything else cannot change it.
    println!("cargo:rerun-if-changed={CONFIG}");
    println!("cargo:rerun-if-changed={C_API_CRATE}/src/embedded/c_api.rs");
    println!("cargo:rerun-if-changed={C_API_CRATE}/src/embedded/c_api");
    println!("cargo:rerun-if-changed=crates/linkrs-core/src/types/c_api.rs");

    if std::env::var_os("CARGO_FEATURE_C_API").is_none() {
        return;
    }

    // cbindgen emits per-item warnings for the `feature = "embedded"` gating of
    // the parsed crate; only surface its output when generation actually fails.
    // Air-gapped build machines may not have cbindgen on PATH. The header is
    // committed, so keep it and continue with a warning instead of failing;
    // drift is still caught by the header-consistency test.
    let output = match Command::new("cbindgen")
        .args(["-c", CONFIG, "-o", HEADER, C_API_CRATE])
        .output()
    {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "cargo:warning=cbindgen not found; keeping committed {HEADER} without regenerating"
            );
            return;
        }
        Err(e) => panic!("failed to run cbindgen while generating {HEADER}: {e}"),
    };

    if !output.status.success() {
        eprint!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        panic!("cbindgen failed while generating {HEADER}");
    }
}
