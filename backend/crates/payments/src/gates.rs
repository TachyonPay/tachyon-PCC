//! Execution safety gates. Defaults are the safe values; unparsable input is
//! treated as the safe value, never as "enabled".

use crate::asset::ChainId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionGates {
    pub dry_run: bool,
    pub enable_mainnet_transfers: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GateError {
    #[error("DRY_RUN is enabled; refusing to broadcast")]
    DryRun,
    #[error("ENABLE_MAINNET_TRANSFERS is false; refusing mainnet value transfer on chain {0:?}")]
    MainnetDisabled(ChainId),
    #[error("operation requires an explicit per-transaction owner approval reference")]
    MissingApproval,
}

impl Default for ExecutionGates {
    fn default() -> Self {
        Self {
            dry_run: true,
            enable_mainnet_transfers: false,
        }
    }
}

impl ExecutionGates {
    /// Build from environment-style lookups. Only the exact string `"false"`
    /// disables `DRY_RUN`; only the exact string `"true"` enables mainnet.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let dry_run = lookup("DRY_RUN").is_none_or(|v| v.trim() != "false");
        let enable_mainnet_transfers = lookup("ENABLE_MAINNET_TRANSFERS").is_some_and(|v| v.trim() == "true");
        Self {
            dry_run,
            enable_mainnet_transfers,
        }
    }

    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Check whether a value-moving operation on `chain` may be broadcast.
    /// `approval_ref` is the id of the owner's explicit, transaction-specific
    /// approval; it is required for any live operation.
    pub fn check_broadcast(&self, chain: ChainId, approval_ref: Option<&str>) -> Result<(), GateError> {
        if self.dry_run {
            return Err(GateError::DryRun);
        }
        if chain.is_mainnet_candidate() && !self.enable_mainnet_transfers {
            return Err(GateError::MainnetDisabled(chain));
        }
        if approval_ref.is_none_or(str::is_empty) {
            return Err(GateError::MissingApproval);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn gates(env: &[(&str, &str)]) -> ExecutionGates {
        let m: HashMap<String, String> = env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        ExecutionGates::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn defaults_are_safe() {
        let g = gates(&[]);
        assert_eq!(g, ExecutionGates::default());
        assert!(g.dry_run);
        assert!(!g.enable_mainnet_transfers);
    }

    #[test]
    fn malformed_values_stay_safe() {
        let g = gates(&[("DRY_RUN", "no"), ("ENABLE_MAINNET_TRANSFERS", "yes")]);
        assert!(g.dry_run);
        assert!(!g.enable_mainnet_transfers);
        let g = gates(&[("DRY_RUN", "0"), ("ENABLE_MAINNET_TRANSFERS", "1")]);
        assert!(g.dry_run);
        assert!(!g.enable_mainnet_transfers);
    }

    #[test]
    fn broadcast_requires_all_gates() {
        let g = gates(&[]);
        assert_eq!(g.check_broadcast(ChainId::BASE, Some("appr-1")), Err(GateError::DryRun));

        let g = gates(&[("DRY_RUN", "false")]);
        assert_eq!(
            g.check_broadcast(ChainId::BASE, Some("appr-1")),
            Err(GateError::MainnetDisabled(ChainId::BASE))
        );
        // Testnet is not mainnet-gated but still needs dry_run off and an approval.
        assert_eq!(
            g.check_broadcast(ChainId::SEPOLIA, None),
            Err(GateError::MissingApproval)
        );
        assert_eq!(
            g.check_broadcast(ChainId::SEPOLIA, Some("")),
            Err(GateError::MissingApproval)
        );
        assert_eq!(g.check_broadcast(ChainId::SEPOLIA, Some("appr-1")), Ok(()));

        let g = gates(&[("DRY_RUN", "false"), ("ENABLE_MAINNET_TRANSFERS", "true")]);
        assert_eq!(g.check_broadcast(ChainId::BASE, None), Err(GateError::MissingApproval));
        assert_eq!(g.check_broadcast(ChainId::BASE, Some("appr-1")), Ok(()));
    }
}
