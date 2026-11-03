//! Writes the Rust-owned internal OpenAPI contract to disk.
//!
//! Usage: `cargo run -p pcc-integration-client --bin export-internal-openapi [-- <out-path>]`
//! Default output: `backend/crates/integration_client/openapi/internal-v1.json`.

use pcc_integration_client::contract::InternalApiDoc;
use std::path::PathBuf;

fn main() {
    let out = std::env::args().nth(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("openapi/internal-v1.json"),
        PathBuf::from,
    );
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).expect("create output directory");
    }
    let json = InternalApiDoc::to_pretty_json();
    std::fs::write(&out, format!("{json}\n")).expect("write openapi json");
    println!("wrote {}", out.display());
}
