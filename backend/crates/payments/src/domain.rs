//! Minimal domain records (shape from the build specification). Storage
//! mapping lives in `pcc-storage`; these are the in-memory/business types.

use crate::amount::BaseUnits;
use crate::asset::{AssetId, ChainId};
use crate::status::{PaymentAttemptStatus, PaymentIntentStatus};
use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
        #[serde(transparent)]
        #[schema(value_type = String, format = Uuid)]
        pub struct $name(pub Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_newtype!(MerchantId);
id_newtype!(PayoutConfigId);
id_newtype!(WalletCredentialId);
id_newtype!(PaymentIntentId);
id_newtype!(PaymentAttemptId);
id_newtype!(AuthorizationId);
id_newtype!(PaymentEventId);
id_newtype!(WebhookDeliveryId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MerchantStatus {
    Active,
    Suspended,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Merchant {
    pub id: MerchantId,
    pub status: MerchantStatus,
    /// Hash of the merchant API key (never the key itself).
    pub api_key_hash: String,
    pub webhook_url: Option<String>,
    /// Reference into the secret store, never the secret.
    pub webhook_secret_ref: Option<String>,
}

/// Merchant-owned payout destination. Snapshotted into every intent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PayoutConfig {
    pub id: PayoutConfigId,
    pub merchant_id: MerchantId,
    pub asset: AssetId,
    #[schema(value_type = String)]
    pub receiving_address: Address,
    pub active: bool,
}

impl PayoutConfig {
    #[must_use]
    pub fn chain_id(&self) -> ChainId {
        self.asset.chain_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WalletCredentialStatus {
    PendingEnrollment,
    Active,
    Revoked,
    Expired,
}

/// A sender EOA with a scoped EIP-7702 permission on one chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WalletCredential {
    pub id: WalletCredentialId,
    #[schema(value_type = String)]
    pub owner_address: Address,
    pub chain_id: ChainId,
    /// Delegation target observed on-chain (EIP-7702 `0xef0100 || address`).
    #[schema(value_type = Option<String>)]
    pub delegation_impl: Option<Address>,
    /// Backend session signer address (public key only).
    #[schema(value_type = Option<String>)]
    pub session_signer: Option<Address>,
    /// Hash of the serialized permission grant.
    pub permissions_hash: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status: WalletCredentialStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PaymentIntent {
    pub id: PaymentIntentId,
    pub merchant_id: MerchantId,
    pub merchant_order_id: String,
    /// Immutable copy of the payout configuration at creation time.
    pub payout_config_snapshot: PayoutConfig,
    pub output_amount_base_units: BaseUnits,
    pub status: PaymentIntentStatus,
    pub expires_at: DateTime<Utc>,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PaymentAttempt {
    pub id: PaymentAttemptId,
    pub payment_intent_id: PaymentIntentId,
    #[schema(value_type = String)]
    pub source_wallet: Address,
    pub source_asset: AssetId,
    /// `amountIn` from the EXACT_OUTPUT quote (includes slippage buffer).
    pub quoted_input: BaseUnits,
    /// Hard cap presented to and authorized by the payer.
    pub max_debit: BaseUnits,
    pub quote_signature: String,
    /// Exact 1Click JSON (immutable). Used to re-verify after restart.
    pub quote_raw_response: String,
    pub quote_expires_at: DateTime<Utc>,
    pub deposit_address: Option<String>,
    pub deposit_memo: Option<String>,
    pub source_tx_hash: Option<String>,
    pub destination_tx_hash: Option<String>,
    /// Owner's per-transaction approval reference (set at execute).
    pub approval_ref: Option<String>,
    pub status: PaymentAttemptStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorization {
    pub id: AuthorizationId,
    pub wallet_credential_id: WalletCredentialId,
    pub payment_attempt_id: PaymentAttemptId,
    pub challenge_id: String,
    /// Hash binding (attempt id, max debit, merchant id, payout snapshot).
    pub amount_bound_hash: String,
    pub verified_at: Option<DateTime<Utc>>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentEvent {
    pub id: PaymentEventId,
    pub payment_intent_id: PaymentIntentId,
    pub r#type: String,
    pub payload_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookDeliveryStatus {
    Pending,
    Delivered,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookDelivery {
    pub id: WebhookDeliveryId,
    pub event_id: PaymentEventId,
    pub attempt: u32,
    pub status: WebhookDeliveryStatus,
    pub next_retry_at: Option<DateTime<Utc>>,
}

/// Compute the authorization binding hash: a TOTP/approval result is only
/// valid for this exact attempt, amount cap, merchant and payout snapshot.
#[must_use]
pub fn amount_bound_hash(attempt: &PaymentAttempt, intent: &PaymentIntent) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"pcc.auth.v1|");
    h.update(attempt.id.to_string().as_bytes());
    h.update(b"|");
    h.update(attempt.max_debit.to_string().as_bytes());
    h.update(b"|");
    h.update(attempt.source_asset.one_click_asset_id.as_bytes());
    h.update(b"|");
    h.update(intent.merchant_id.to_string().as_bytes());
    h.update(b"|");
    h.update(intent.payout_config_snapshot.receiving_address.to_string().as_bytes());
    h.update(b"|");
    h.update(intent.payout_config_snapshot.asset.one_click_asset_id.as_bytes());
    h.update(b"|");
    h.update(intent.output_amount_base_units.to_string().as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::TokenRef;

    fn fixtures() -> (PaymentIntent, PaymentAttempt) {
        let payout = PayoutConfig {
            id: PayoutConfigId::new(),
            merchant_id: MerchantId::new(),
            asset: AssetId {
                chain_id: ChainId::ARBITRUM,
                token: TokenRef::Erc20 {
                    address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831".parse().unwrap(),
                },
                decimals: 6,
                one_click_asset_id: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near".into(),
            },
            receiving_address: "0x1111111111111111111111111111111111111111".parse().unwrap(),
            active: true,
        };
        let intent = PaymentIntent {
            id: PaymentIntentId::new(),
            merchant_id: payout.merchant_id,
            merchant_order_id: "order-1".into(),
            payout_config_snapshot: payout,
            output_amount_base_units: BaseUnits::from_u128(1_000_000),
            status: PaymentIntentStatus::RequiresPayment,
            expires_at: Utc::now(),
            idempotency_key: "idem-1".into(),
        };
        let attempt = PaymentAttempt {
            id: PaymentAttemptId::new(),
            payment_intent_id: intent.id,
            source_wallet: "0x2222222222222222222222222222222222222222".parse().unwrap(),
            source_asset: AssetId {
                chain_id: ChainId::BASE,
                token: TokenRef::Erc20 {
                    address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().unwrap(),
                },
                decimals: 6,
                one_click_asset_id: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near".into(),
            },
            quoted_input: BaseUnits::from_u128(1_010_000),
            max_debit: BaseUnits::from_u128(1_010_000),
            quote_signature: "ed25519:sig".into(),
            quote_raw_response: "{}".into(),
            quote_expires_at: Utc::now(),
            deposit_address: None,
            deposit_memo: None,
            source_tx_hash: None,
            destination_tx_hash: None,
            approval_ref: None,
            status: PaymentAttemptStatus::Quoted,
        };
        (intent, attempt)
    }

    #[test]
    fn binding_hash_changes_when_amount_or_merchant_changes() {
        let (intent, attempt) = fixtures();
        let base = amount_bound_hash(&attempt, &intent);

        let mut a2 = attempt.clone();
        a2.max_debit = BaseUnits::from_u128(1_010_001);
        assert_ne!(amount_bound_hash(&a2, &intent), base);

        let mut i2 = intent.clone();
        i2.payout_config_snapshot.receiving_address = "0x3333333333333333333333333333333333333333".parse().unwrap();
        assert_ne!(amount_bound_hash(&attempt, &i2), base);

        let mut i3 = intent.clone();
        i3.output_amount_base_units = BaseUnits::from_u128(2_000_000);
        assert_ne!(amount_bound_hash(&attempt, &i3), base);

        assert_eq!(amount_bound_hash(&attempt, &intent), base);
    }

    #[test]
    fn ids_serialize_as_strings() {
        let id = PaymentIntentId::new();
        let s = serde_json::to_string(&id).unwrap();
        assert!(s.starts_with('"'));
    }
}
