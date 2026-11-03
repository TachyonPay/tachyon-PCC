//! Internal boundary between the Rust backend and the private TypeScript
//! integration worker (`services/integrations-ts`).
//!
//! Rust owns the contract: the DTOs in [`contract`] are rendered to OpenAPI
//! (`openapi/internal-v1.json`) and TypeScript types are generated from that
//! file. A drift between the Rust types and the committed JSON fails CI.
//!
//! The worker is never public. The client always sends a bearer token, a
//! per-request id, and a bounded timeout, and it treats every response as
//! untrusted input: full deserialization + business validation happens in
//! Rust regardless of what the worker claims.

pub mod client;
pub mod contract;

pub use client::{IntegrationClient, IntegrationClientConfig};
