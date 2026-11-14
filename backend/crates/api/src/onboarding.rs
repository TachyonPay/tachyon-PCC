//! Payer onboarding status (M11): whether the signed-in wallet already
//! delegates to Kernel v3.3 via EIP-7702 on each source chain. Read-only.
//! The page asks the wallet to install Kernel only when the account has no
//! code, and never replaces an existing delegation.

use crate::settle::RpcDestinationVerifier;
use crate::{fail, ApiResult, AppState, ErrorBody};
use alloy_primitives::{address, Address};
use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use pcc_payments::asset::ChainId;
use serde::Serialize;
use utoipa::ToSchema;

/// Kernel v3.3 EIP-7702 implementation (ZeroDev `KERNEL_7702_DELEGATION_ADDRESS`).
pub const KERNEL_7702_IMPLEMENTATION: Address = address!("d6CEDDe84be40893d153Be9d467CD6aD37875b28");
const DELEGATION_PREFIX: [u8; 3] = [0xef, 0x01, 0x00];

#[async_trait]
pub trait CodeReader: Send + Sync {
    async fn code(&self, chain: ChainId, account: Address) -> Result<Vec<u8>, String>;
}

#[async_trait]
impl CodeReader for RpcDestinationVerifier {
    async fn code(&self, chain: ChainId, account: Address) -> Result<Vec<u8>, String> {
        let hex = self
            .call(chain, "eth_getCode", serde_json::json!([account.to_string(), "latest"]))
            .await?;
        let hex = hex.as_str().ok_or("eth_getCode returned no string")?;
        hex::decode(hex.trim_start_matches("0x")).map_err(|_| "eth_getCode returned invalid hex".to_owned())
    }
}

#[derive(Serialize, ToSchema, Debug, PartialEq, Eq)]
pub struct ChainDelegationView {
    pub chain_id: u64,
    /// `none`, `delegated` or `contract`.
    pub state: String,
    pub target: Option<String>,
    /// True only for a Kernel v3.3 delegation: session grants work as-is.
    pub ready: bool,
    pub detail: String,
}

#[derive(Serialize, ToSchema)]
pub struct DelegationStatusResponse {
    pub address: String,
    pub kernel_implementation: String,
    pub chains: Vec<ChainDelegationView>,
}

#[must_use]
pub fn classify(chain: ChainId, code: &[u8]) -> ChainDelegationView {
    let view = |state: &str, target: Option<Address>, ready: bool, detail: &str| ChainDelegationView {
        chain_id: chain.0,
        state: state.into(),
        target: target.map(|t| t.to_string()),
        ready,
        detail: detail.into(),
    };
    if code.is_empty() {
        return view(
            "none",
            None,
            false,
            "Plain EOA. Upgrade it to Kernel v3.3 with an EIP-7702 authorization from your wallet before paying from this chain.",
        );
    }
    if code.len() == 23 && code[..3] == DELEGATION_PREFIX {
        let target = Address::from_slice(&code[3..]);
        return if target == KERNEL_7702_IMPLEMENTATION {
            view("delegated", Some(target), true, "Kernel v3.3 delegation found.")
        } else {
            view(
                "delegated",
                Some(target),
                false,
                "Delegated to a different smart-account implementation; this checkout will not replace it.",
            )
        };
    }
    view("contract", None, false, "Contract account, not an EOA; not supported.")
}

#[utoipa::path(get, path = "/v1/checkout/{session_id}/delegation", tag = "checkout",
    params(("session_id" = String, Path, description = "Public checkout session id")),
    responses((status = 200, body = DelegationStatusResponse), (status = 401, body = ErrorBody),
              (status = 503, body = ErrorBody)))]
pub async fn delegation_status(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<DelegationStatusResponse>> {
    let (_, owner) = state
        .store
        .wallet_for_session(&session_id)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "siwe_required", "SIWE first"))?;
    let reader = state.code_reader.clone().ok_or_else(|| {
        fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "unconfigured",
            "chain reads not configured",
        )
    })?;
    let mut chains = Vec::new();
    for chain in crate::checkout::SOURCE_CHAIN_IDS {
        let view = match reader.code(chain, owner).await {
            Ok(code) => classify(chain, &code),
            Err(e) => ChainDelegationView {
                chain_id: chain.0,
                state: "unknown".into(),
                target: None,
                ready: false,
                detail: format!("could not read account code: {e}"),
            },
        };
        chains.push(view);
    }
    Ok(Json(DelegationStatusResponse {
        address: owner.to_string(),
        kernel_implementation: KERNEL_7702_IMPLEMENTATION.to_string(),
        chains,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn designator(target: Address) -> Vec<u8> {
        let mut c = DELEGATION_PREFIX.to_vec();
        c.extend_from_slice(target.as_slice());
        c
    }

    #[test]
    fn classifies_the_shared_delegation_vectors() {
        let none = classify(ChainId::BASE, &[]);
        assert_eq!((none.state.as_str(), none.ready), ("none", false));

        let ours = classify(ChainId::BASE, &designator(KERNEL_7702_IMPLEMENTATION));
        assert_eq!((ours.state.as_str(), ours.ready), ("delegated", true));

        let foreign = classify(ChainId::ARBITRUM, &designator(Address::repeat_byte(0x63)));
        assert_eq!((foreign.state.as_str(), foreign.ready), ("delegated", false));
        assert_eq!(foreign.target, Some(Address::repeat_byte(0x63).to_string()));

        let mut long = designator(KERNEL_7702_IMPLEMENTATION);
        long.push(0);
        assert_eq!(classify(ChainId::BASE, &long).state, "contract");
        assert_eq!(classify(ChainId::BASE, &[0x60, 0x80]).state, "contract");
    }
}
