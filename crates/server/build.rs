//! Rebuilds the server when the built web UI changes (feature `web-ui`, ADR-0015).
//!
//! In release builds rust-embed compiles every file of `web/dist` into the binary with `include_bytes!`, so cargo
//! notices changed files but not a `web/dist` that appeared after the last build or files that were only added.
//! Watching the folder closes that gap. Without the feature nothing is embedded and nothing is watched.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_WEB_UI").is_some() {
        println!("cargo::rerun-if-changed=../../web/dist");
    }
}
