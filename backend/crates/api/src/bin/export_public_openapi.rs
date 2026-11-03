//! Writes the Rust-owned public OpenAPI contract to disk.
//!
//! Usage: `cargo run -p pcc-api --bin export-public-openapi [-- <out-path>]`
//! Default output: `backend/crates/api/openapi/public-v1.json`.

use pcc_api::PublicApiDoc;
use std::path::PathBuf;

fn main() {
    let out = std::env::args().nth(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("openapi/public-v1.json"),
        PathBuf::from,
    );
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).expect("create output directory");
    }
    let json = PublicApiDoc::to_pretty_json();
    std::fs::write(&out, format!("{json}\n")).expect("write openapi json");
    println!("wrote {}", out.display());
}
