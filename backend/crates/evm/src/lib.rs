//! EVM building blocks used by the plain-EOA `SourceExecutor` (M2) and by
//! wallet-credential inspection (M3).
//!
//! No network access lives here yet; the Alloy provider + signer executor is
//! added in M2 behind the execution gates. Keeping this crate pure makes the
//! calldata and delegation logic unit-testable without an RPC.

pub mod delegation;
pub mod transfer;
