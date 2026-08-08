//! Build script for `sdkt-cli` (M39).
//!
//! Captures optional build provenance into `cargo:` rustc-env directives so the
//! binary can append a commit/date line to `sdkt --version` *only* when the
//! `provenance` feature is enabled at compile time.
//!
//! Nothing here affects reproducibility by default: when `provenance` is off
//! (the default), these env vars are simply never read and the version string
//! is just `CARGO_PKG_VERSION`. The release workflow opts in with
//! `--features provenance` and supplies `SDKT_GIT_COMMIT` / `SDKT_BUILD_DATE`.

fn main() {
    // Out dir is provided by Cargo; re-exporting is unnecessary for our use.
    println!("cargo:rerun-if-env-changed=SDKT_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=SDKT_BUILD_DATE");
    println!("cargo:rerun-if-changed=build.rs");
}
