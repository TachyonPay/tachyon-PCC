//! Payment domain for the Private Crypto Checkout backend.
//!
//! This crate is deliberately free of I/O. It owns:
//! - exact base-unit amounts (`U256`, never floats),
//! - asset identity (`chainId` + token + decimals + 1Click asset id),
//! - the payment state machine,
//! - quote validation rules applied *after* the TS worker has verified the
//!   1Click signature,
//! - execution safety gates (`DRY_RUN`, `ENABLE_MAINNET_TRANSFERS`),
//! - the adapter traits implemented by the EVM executor and the TS
//!   integration worker client.

pub mod amount;
pub mod asset;
pub mod domain;
pub mod gates;
pub mod quote;
pub mod status;
pub mod traits;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub use amount::BaseUnits;
pub use asset::{AssetId, ChainId, TokenRef};
pub use gates::ExecutionGates;
