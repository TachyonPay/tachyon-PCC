//! Asset identity. A ticker is never an identity.

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// EIP-155 chain id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct ChainId(pub u64);

impl ChainId {
    pub const ETHEREUM: Self = Self(1);
    pub const OPTIMISM: Self = Self(10);
    pub const BSC: Self = Self(56);
    pub const GNOSIS: Self = Self(100);
    pub const POLYGON: Self = Self(137);
    pub const BASE: Self = Self(8453);
    pub const ARBITRUM: Self = Self(42161);
    pub const AVALANCHE: Self = Self(43114);
    pub const SCROLL: Self = Self(534_352);
    pub const SEPOLIA: Self = Self(11_155_111);
    /// Base Sepolia. 7702 account tests only; never a 1Click settlement chain.
    pub const BASE_SEPOLIA: Self = Self(84_532);
    /// Anvil/Hardhat local chain. Never a mainnet candidate.
    pub const ANVIL: Self = Self(31_337);

    /// Map a 1Click `blockchain` slug to an EVM chain id.
    ///
    /// Only slugs with a verified EIP-155 mapping are included; anything else
    /// returns `None` and is filtered out of the eligible route set. Extend
    /// only with evidence recorded in `docs/sdk-compatibility.md`.
    #[must_use]
    pub fn from_one_click_blockchain(slug: &str) -> Option<Self> {
        Some(match slug {
            "eth" => Self::ETHEREUM,
            "op" => Self::OPTIMISM,
            "bsc" => Self::BSC,
            "gnosis" => Self::GNOSIS,
            "pol" => Self::POLYGON,
            "base" => Self::BASE,
            "arb" => Self::ARBITRUM,
            "avax" => Self::AVALANCHE,
            "scroll" => Self::SCROLL,
            _ => return None,
        })
    }

    /// Inverse of [`Self::from_one_click_blockchain`].
    #[must_use]
    pub fn one_click_blockchain(self) -> Option<&'static str> {
        Some(match self {
            Self::ETHEREUM => "eth",
            Self::OPTIMISM => "op",
            Self::BSC => "bsc",
            Self::GNOSIS => "gnosis",
            Self::POLYGON => "pol",
            Self::BASE => "base",
            Self::ARBITRUM => "arb",
            Self::AVALANCHE => "avax",
            Self::SCROLL => "scroll",
            _ => return None,
        })
    }

    /// Chains where the MVP is allowed to move value. Testnets are never here.
    #[must_use]
    pub fn is_mainnet_candidate(self) -> bool {
        self.one_click_blockchain().is_some()
    }
}

/// Native coin or ERC-20 contract on a given chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TokenRef {
    Native,
    Erc20 {
        #[schema(value_type = String, example = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913")]
        address: Address,
    },
}

/// Full asset identity used everywhere in the backend.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct AssetId {
    pub chain_id: ChainId,
    pub token: TokenRef,
    pub decimals: u8,
    /// Exact 1Click asset id, e.g. `nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near`.
    pub one_click_asset_id: String,
}

impl AssetId {
    /// Parse a 1Click token entry into an `AssetId`, returning `None` for
    /// non-EVM or unmapped chains.
    #[must_use]
    pub fn from_one_click_token(
        blockchain: &str,
        contract_address: Option<&str>,
        decimals: u8,
        asset_id: &str,
    ) -> Option<Self> {
        let chain_id = ChainId::from_one_click_blockchain(blockchain)?;
        let token = match contract_address {
            None | Some("" | "native") => TokenRef::Native,
            Some(addr) => TokenRef::Erc20 {
                address: addr.parse().ok()?,
            },
        };
        Some(Self {
            chain_id,
            token,
            decimals,
            one_click_asset_id: asset_id.to_owned(),
        })
    }

    /// Circle USDC on Base. MVP payer source asset for hosted checkout.
    #[must_use]
    pub fn usdc_base() -> Self {
        Self {
            chain_id: ChainId::BASE,
            token: TokenRef::Erc20 {
                address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".parse().expect("usdc base"),
            },
            decimals: 6,
            one_click_asset_id: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near".into(),
        }
    }

    /// Circle USDC on Arbitrum. Default demo merchant payout asset.
    #[must_use]
    pub fn usdc_arbitrum() -> Self {
        Self {
            chain_id: ChainId::ARBITRUM,
            token: TokenRef::Erc20 {
                address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831".parse().expect("usdc arb"),
            },
            decimals: 6,
            one_click_asset_id: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_mapping_roundtrips() {
        for slug in ["eth", "op", "bsc", "gnosis", "pol", "base", "arb", "avax", "scroll"] {
            let id = ChainId::from_one_click_blockchain(slug).unwrap();
            assert_eq!(id.one_click_blockchain(), Some(slug));
        }
        assert_eq!(ChainId::from_one_click_blockchain("sol"), None);
        assert_eq!(ChainId::from_one_click_blockchain("btc"), None);
        assert_eq!(ChainId::from_one_click_blockchain("near"), None);
        assert_eq!(ChainId::SEPOLIA.one_click_blockchain(), None);
        assert!(!ChainId::SEPOLIA.is_mainnet_candidate());
        assert_eq!(ChainId::BASE_SEPOLIA.one_click_blockchain(), None);
        assert!(!ChainId::BASE_SEPOLIA.is_mainnet_candidate());
    }

    #[test]
    fn parses_evm_token_entries() {
        let usdc_base = AssetId::from_one_click_token(
            "base",
            Some("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"),
            6,
            "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near",
        )
        .unwrap();
        assert_eq!(usdc_base.chain_id, ChainId::BASE);
        assert!(matches!(usdc_base.token, TokenRef::Erc20 { .. }));
        assert_eq!(usdc_base.decimals, 6);

        let eth_native = AssetId::from_one_click_token("eth", None, 18, "nep141:eth.omft.near").unwrap();
        assert_eq!(eth_native.token, TokenRef::Native);

        assert!(AssetId::from_one_click_token("sol", None, 9, "nep141:sol.omft.near").is_none());
        assert!(AssetId::from_one_click_token("base", Some("not-an-address"), 6, "x").is_none());
    }

    #[test]
    fn same_ticker_different_chain_is_different_asset() {
        let a =
            AssetId::from_one_click_token("base", Some("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"), 6, "a").unwrap();
        let b =
            AssetId::from_one_click_token("arb", Some("0xaf88d065e77c8cc2239327c5edb3a432268e5831"), 6, "b").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn well_known_usdc_ids_match_1click() {
        assert_eq!(
            AssetId::usdc_base().one_click_asset_id,
            "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near"
        );
        assert_eq!(
            AssetId::usdc_arbitrum().one_click_asset_id,
            "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near"
        );
        assert_ne!(AssetId::usdc_base(), AssetId::usdc_arbitrum());
    }
}
