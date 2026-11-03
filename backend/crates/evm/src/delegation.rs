//! EIP-7702 delegation inspection.
//!
//! Per EIP-7702, a delegated EOA's code is exactly `0xef0100 || address`
//! (23 bytes). Before onboarding a wallet we read its code and refuse to
//! overwrite an unfamiliar delegation without explicit owner approval.

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

pub const DELEGATION_PREFIX: [u8; 3] = [0xef, 0x01, 0x00];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DelegationState {
    /// Plain EOA: no code.
    None,
    /// EIP-7702 delegation to `target`.
    Delegated { target: Address },
    /// Account has code that is not a 7702 designator (contract account or
    /// unknown). Never treat as an EOA.
    NonDelegationCode { code_len: usize },
}

#[must_use]
pub fn parse_delegation(code: &[u8]) -> DelegationState {
    if code.is_empty() {
        return DelegationState::None;
    }
    if code.len() == 23 && code[..3] == DELEGATION_PREFIX {
        return DelegationState::Delegated {
            target: Address::from_slice(&code[3..]),
        };
    }
    DelegationState::NonDelegationCode { code_len: code.len() }
}

/// Decision helper: may we install `desired` on an account currently in `state`?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnboardDecision {
    /// No delegation present; install allowed (after owner signature).
    Install,
    /// Already delegated to the exact implementation we expect; reuse it.
    AlreadyCompatible,
    /// Delegated to something else; requires explicit owner review/approval.
    RequiresOwnerReview { current: Address },
    /// Not an EOA; refuse.
    Refuse,
}

#[must_use]
pub fn onboard_decision(state: DelegationState, desired: Address, known_compatible: &[Address]) -> OnboardDecision {
    match state {
        DelegationState::None => OnboardDecision::Install,
        DelegationState::Delegated { target } if target == desired || known_compatible.contains(&target) => {
            OnboardDecision::AlreadyCompatible
        }
        DelegationState::Delegated { target } => OnboardDecision::RequiresOwnerReview { current: target },
        DelegationState::NonDelegationCode { .. } => OnboardDecision::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }

    #[test]
    fn parses_designator() {
        let mut code = DELEGATION_PREFIX.to_vec();
        code.extend_from_slice(addr(0xAB).as_slice());
        assert_eq!(
            parse_delegation(&code),
            DelegationState::Delegated { target: addr(0xAB) }
        );
    }

    #[test]
    fn empty_is_eoa_and_other_code_is_not() {
        assert_eq!(parse_delegation(&[]), DelegationState::None);
        assert_eq!(
            parse_delegation(&[0x60, 0x80, 0x60, 0x40]),
            DelegationState::NonDelegationCode { code_len: 4 }
        );
        // Right prefix but wrong length must not be treated as a delegation.
        let mut short = DELEGATION_PREFIX.to_vec();
        short.extend_from_slice(&[0u8; 19]);
        assert!(matches!(
            parse_delegation(&short),
            DelegationState::NonDelegationCode { .. }
        ));
        // ef01 followed by non-zero version byte is not a 7702 designator.
        let mut wrong_version = vec![0xef, 0x01, 0x01];
        wrong_version.extend_from_slice(&[0u8; 20]);
        assert!(matches!(
            parse_delegation(&wrong_version),
            DelegationState::NonDelegationCode { .. }
        ));
    }

    #[test]
    fn onboarding_decisions() {
        let kernel = addr(0x11);
        let other = addr(0x22);
        assert_eq!(
            onboard_decision(DelegationState::None, kernel, &[]),
            OnboardDecision::Install
        );
        assert_eq!(
            onboard_decision(DelegationState::Delegated { target: kernel }, kernel, &[]),
            OnboardDecision::AlreadyCompatible
        );
        assert_eq!(
            onboard_decision(DelegationState::Delegated { target: other }, kernel, &[other]),
            OnboardDecision::AlreadyCompatible
        );
        assert_eq!(
            onboard_decision(DelegationState::Delegated { target: other }, kernel, &[]),
            OnboardDecision::RequiresOwnerReview { current: other }
        );
        assert_eq!(
            onboard_decision(DelegationState::NonDelegationCode { code_len: 100 }, kernel, &[]),
            OnboardDecision::Refuse
        );
    }
}
