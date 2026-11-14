/** Current NEAR Intents assets (1Click /v0/tokens, 2026-11-15). Deprecated ids are omitted. */
export interface PayoutAsset {
  network: string;
  chainId: number;
  symbol: string;
  decimals: number;
  token: string | null;
  assetId: string;
}

export const PAYOUT_ASSETS: readonly PayoutAsset[] = [
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "USDC",
    decimals: 6,
    token: "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
    assetId: "nep141:eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near",
  },
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "USDT",
    decimals: 6,
    token: "0xdac17f958d2ee523a2206206994597c13d831ec7",
    assetId: "nep141:eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near",
  },
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "DAI",
    decimals: 18,
    token: "0x6b175474e89094c44da98b954eedeac495271d0f",
    assetId: "nep141:eth-0x6b175474e89094c44da98b954eedeac495271d0f.omft.near",
  },
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "ETH",
    decimals: 18,
    token: null,
    assetId: "nep141:eth.omft.near",
  },
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "WETH",
    decimals: 18,
    token: "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2",
    assetId: "nep141:eth-0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2.omft.near",
  },
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "AAVE",
    decimals: 18,
    token: "0x7fc66500c84a76ad7e9c93437bfc5ac33e2ddae9",
    assetId: "nep141:eth-0x7fc66500c84a76ad7e9c93437bfc5ac33e2ddae9.omft.near",
  },
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "LINK",
    decimals: 18,
    token: "0x514910771af9ca656af840dff83e8264ecf986ca",
    assetId: "nep141:eth-0x514910771af9ca656af840dff83e8264ecf986ca.omft.near",
  },
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "UNI",
    decimals: 18,
    token: "0x1f9840a85d5af5bf1d1762f925bdaddc4201f984",
    assetId: "nep141:eth-0x1f9840a85d5af5bf1d1762f925bdaddc4201f984.omft.near",
  },
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "WBTC",
    decimals: 8,
    token: "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599",
    assetId: "nep141:eth-0x2260fac5e5542a773aa44fbcfedf7c193bc2c599.omft.near",
  },
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "cbBTC",
    decimals: 8,
    token: "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf",
    assetId: "nep141:eth-0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf.omft.near",
  },
  {
    network: "Ethereum",
    chainId: 1,
    symbol: "wstETH",
    decimals: 18,
    token: "0x7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0",
    assetId: "nep141:eth-0x7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0.omft.near",
  },
  {
    network: "Optimism",
    chainId: 10,
    symbol: "USDC",
    decimals: 6,
    token: "0x0b2c639c533813f4aa9d7837caf62653d097ff85",
    assetId: "nep245:v2_1.omni.hot.tg:10_A2ewyUyDp6qsue1jqZsGypkCxRJ",
  },
  {
    network: "Optimism",
    chainId: 10,
    symbol: "USDT",
    decimals: 6,
    token: "0x94b008aa00579c1307b0ef2c499ad98a8ce58e58",
    assetId: "nep245:v2_1.omni.hot.tg:10_359RPSJVdTxwTJT9TyGssr2rFoWo",
  },
  {
    network: "Optimism",
    chainId: 10,
    symbol: "USDT0",
    decimals: 6,
    token: "0x01bff41798a0bcf287b996046ca68b395dbc1071",
    assetId: "nep245:v2_1.omni.hot.tg:10_2R1RXDBxCyJTeMEsdXydh7xsHmz",
  },
  {
    network: "Optimism",
    chainId: 10,
    symbol: "ETH",
    decimals: 18,
    token: null,
    assetId: "nep245:v2_1.omni.hot.tg:10_11111111111111111111",
  },
  {
    network: "Optimism",
    chainId: 10,
    symbol: "WETH",
    decimals: 18,
    token: "0x4200000000000000000000000000000000000006",
    assetId: "nep245:v2_1.omni.hot.tg:10_vLAiSt9KfUGKpw5cD3vsSyNYBn5",
  },
  {
    network: "Optimism",
    chainId: 10,
    symbol: "OP",
    decimals: 18,
    token: "0x4200000000000000000000000000000000000042",
    assetId: "nep245:v2_1.omni.hot.tg:10_vLAiSt9KfUGKpw5cD3vsSyNYBo7",
  },
  {
    network: "Base",
    chainId: 8453,
    symbol: "USDC",
    decimals: 6,
    token: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
    assetId: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near",
  },
  {
    network: "Base",
    chainId: 8453,
    symbol: "ETH",
    decimals: 18,
    token: null,
    assetId: "nep141:base.omft.near",
  },
  {
    network: "Base",
    chainId: 8453,
    symbol: "WETH",
    decimals: 18,
    token: "0x4200000000000000000000000000000000000006",
    assetId: "nep141:base-0x4200000000000000000000000000000000000006.omft.near",
  },
  {
    network: "Base",
    chainId: 8453,
    symbol: "cbBTC",
    decimals: 8,
    token: "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf",
    assetId: "nep141:base-0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf.omft.near",
  },
  {
    network: "Arbitrum",
    chainId: 42161,
    symbol: "USDC",
    decimals: 6,
    token: "0xaf88d065e77c8cc2239327c5edb3a432268e5831",
    assetId: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near",
  },
  {
    network: "Arbitrum",
    chainId: 42161,
    symbol: "USDT0",
    decimals: 6,
    token: "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9",
    assetId: "nep141:arb-0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9.omft.near",
  },
  {
    network: "Arbitrum",
    chainId: 42161,
    symbol: "ETH",
    decimals: 18,
    token: null,
    assetId: "nep141:arb.omft.near",
  },
  {
    network: "Arbitrum",
    chainId: 42161,
    symbol: "WETH",
    decimals: 18,
    token: "0x82af49447d8a07e3bd95bd0d56f35241523fbab1",
    assetId: "nep141:arb-0x82af49447d8a07e3bd95bd0d56f35241523fbab1.omft.near",
  },
  {
    network: "Arbitrum",
    chainId: 42161,
    symbol: "ARB",
    decimals: 18,
    token: "0x912ce59144191c1204e64559fe8253a0e49e6548",
    assetId: "nep141:arb-0x912ce59144191c1204e64559fe8253a0e49e6548.omft.near",
  },
  {
    network: "Polygon",
    chainId: 137,
    symbol: "USDC",
    decimals: 6,
    token: "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359",
    assetId: "nep141:pol-0x3c499c542cef5e3811e1192ce70d8cc03d5c3359.omdep.near",
  },
  {
    network: "Polygon",
    chainId: 137,
    symbol: "USDT",
    decimals: 6,
    token: "0xc2132d05d31c914a87c6611c10748aeb04b58e8f",
    assetId: "nep141:pol-0xc2132d05d31c914a87c6611c10748aeb04b58e8f.omdep.near",
  },
  {
    network: "Polygon",
    chainId: 137,
    symbol: "WETH",
    decimals: 18,
    token: "0x7ceb23fd6bc0add59e62ac25578270cff1b9f619",
    assetId: "nep141:pol-0x7ceb23fd6bc0add59e62ac25578270cff1b9f619.omdep.near",
  },
  {
    network: "Polygon",
    chainId: 137,
    symbol: "POL",
    decimals: 18,
    token: null,
    assetId: "nep141:pol.omdep.near",
  },
  {
    network: "BNB Chain",
    chainId: 56,
    symbol: "USDC",
    decimals: 18,
    token: "0x8ac76a51cc950d9822d68b83fe1ad97b32cd580d",
    assetId: "nep141:bnb-0x8ac76a51cc950d9822d68b83fe1ad97b32cd580d.omdep.near",
  },
  {
    network: "BNB Chain",
    chainId: 56,
    symbol: "USDT",
    decimals: 18,
    token: "0x55d398326f99059ff775485246999027b3197955",
    assetId: "nep141:bnb-0x55d398326f99059ff775485246999027b3197955.omdep.near",
  },
  {
    network: "BNB Chain",
    chainId: 56,
    symbol: "BNB",
    decimals: 18,
    token: null,
    assetId: "nep141:bnb.omdep.near",
  },
  {
    network: "Avalanche",
    chainId: 43114,
    symbol: "USDC",
    decimals: 6,
    token: "0xb97ef9ef8734c71904d8002f8b6bc66dd9c48a6e",
    assetId: "nep141:avax-0xb97ef9ef8734c71904d8002f8b6bc66dd9c48a6e.omdep.near",
  },
  {
    network: "Avalanche",
    chainId: 43114,
    symbol: "USDT",
    decimals: 6,
    token: "0x9702230a8ea53601f5cd2dc00fdbc13d4df4a8c7",
    assetId: "nep141:avax-0x9702230a8ea53601f5cd2dc00fdbc13d4df4a8c7.omdep.near",
  },
  {
    network: "Avalanche",
    chainId: 43114,
    symbol: "AVAX",
    decimals: 18,
    token: null,
    assetId: "nep141:avax.omdep.near",
  },
  {
    network: "Gnosis",
    chainId: 100,
    symbol: "USDC",
    decimals: 6,
    token: "0x2a22f9c3b484c3629090feed35f17ff8f88f76f0",
    assetId: "nep141:gnosis-0x2a22f9c3b484c3629090feed35f17ff8f88f76f0.omft.near",
  },
  {
    network: "Gnosis",
    chainId: 100,
    symbol: "USDT",
    decimals: 6,
    token: "0x4ecaba5870353805a9f068101a40e0f32ed605c6",
    assetId: "nep141:gnosis-0x4ecaba5870353805a9f068101a40e0f32ed605c6.omft.near",
  },
  {
    network: "Gnosis",
    chainId: 100,
    symbol: "WETH",
    decimals: 18,
    token: "0x6a023ccd1ff6f2045c3309768ead9e68f978f6e1",
    assetId: "nep141:gnosis-0x6a023ccd1ff6f2045c3309768ead9e68f978f6e1.omft.near",
  },
  {
    network: "Gnosis",
    chainId: 100,
    symbol: "xDAI",
    decimals: 18,
    token: null,
    assetId: "nep141:gnosis.omft.near",
  },
  {
    network: "Scroll",
    chainId: 534352,
    symbol: "USDT",
    decimals: 6,
    token: "0xf55bec9cafdbe8730f096aa55dad6d22d44099df",
    assetId: "nep245:v2_1.omni.hot.tg:534352_4RG3Q2wFsMQmd45m5m89RjsLfupA",
  },
  {
    network: "Scroll",
    chainId: 534352,
    symbol: "ETH",
    decimals: 18,
    token: null,
    assetId: "nep245:v2_1.omni.hot.tg:534352_11111111111111111111",
  },
];

export const DEFAULT_PAYOUT_ASSET: PayoutAsset =
  PAYOUT_ASSETS.find((a) => a.chainId === 42161 && a.symbol === "USDC") ?? PAYOUT_ASSETS[0]!;

export function matchPayout(chainId: number, assetId: string): PayoutAsset {
  return (
    PAYOUT_ASSETS.find((a) => a.assetId === assetId) ??
    PAYOUT_ASSETS.find((a) => a.chainId === chainId) ??
    DEFAULT_PAYOUT_ASSET
  );
}

export function payoutAssetFor(chainId: number, symbol: string): PayoutAsset | undefined {
  return (
    PAYOUT_ASSETS.find((a) => a.chainId === chainId && a.symbol === symbol) ??
    PAYOUT_ASSETS.find((a) => a.chainId === chainId && a.symbol === "USDC") ??
    PAYOUT_ASSETS.find((a) => a.chainId === chainId)
  );
}
