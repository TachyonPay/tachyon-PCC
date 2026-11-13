//! EVM building blocks and the plain-EOA [`SourceExecutor`] (M2).
//!
//! Calldata construction and EIP-7702 inspection stay pure (no network). The
//! Alloy executor talks to an RPC, but every broadcast is gated by
//! [`pcc_payments::ExecutionGates`] and a persist-then-send ledger.

pub mod delegation;
pub mod envfile;
pub mod executor;
pub mod flow;
pub mod keys;
pub mod ledger;
pub mod plan;
pub mod transfer;

pub use executor::AlloyEoaExecutor;
pub use flow::{
    execute_approved, prepare_deposit, redacted_preflight, watch_until, FlowError, PreflightReport, PrepareParams,
};
pub use keys::load_signer;
pub use ledger::FileLedger;
pub use plan::{assert_plan_matches_quote, plan_from_quote};
pub use transfer::{build_deposit_call, decode_erc20_transfer, encode_erc20_transfer, CallError, DepositCall};

#[cfg(test)]
mod anvil_tests;
#[cfg(test)]
mod flow_tests;
