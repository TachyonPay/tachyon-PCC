/**
 * Pinned account stack for the EIP-7702 path. One Kernel version, one
 * EntryPoint, one SDK major. Do not mix with Kernel v4 docs (EntryPoint v0.9)
 * which the kernel repo `dev` README currently describes.
 */
import {
  KERNEL_V3_3,
  KERNEL_7702_DELEGATION_ADDRESS as ZD_7702_DELEGATION,
  getEntryPoint,
} from "@zerodev/sdk/constants";
import type { Address } from "viem";

export const KERNEL_VERSION = KERNEL_V3_3;
export const ENTRY_POINT = getEntryPoint("0.7");

/** Kernel v3.3 7702 designator. The only implementation we will install. */
export const KERNEL_7702_DELEGATION_ADDRESS: Address = ZD_7702_DELEGATION;

/** Chains where ZeroDev documents 7702 support and we have reviewed RPCs. */
export const SUPPORTED_7702_CHAINS = {
  sepolia: 11155111,
  baseSepolia: 84532,
  ethereum: 1,
  optimism: 10,
  base: 8453,
  arbitrum: 42161,
} as const;

/** Circle USDC on Base Sepolia (6 decimals). Testnet only. */
export const USDC_BASE_SEPOLIA = "0x036CbD53842c5426634e7929541eC2318f3dCF7e" as Address;
/** Circle USDC on Base (6 decimals). */
export const USDC_BASE = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" as Address;

export type Supported7702Chain = (typeof SUPPORTED_7702_CHAINS)[keyof typeof SUPPORTED_7702_CHAINS];

export const MAINNET_7702_CHAIN_IDS: readonly number[] = [
  SUPPORTED_7702_CHAINS.ethereum,
  SUPPORTED_7702_CHAINS.optimism,
  SUPPORTED_7702_CHAINS.base,
  SUPPORTED_7702_CHAINS.arbitrum,
];

/** Worst-case gas cost a checkout session UserOp may spend (0.0003 ETH). */
export const CHECKOUT_MAX_GAS_WEI = 300_000_000_000_000n;

/** Inclusive UserOp gas *limit* for our local admission check. */
export const SESSION_MAX_GAS = 500_000n;
/** ZeroDev gas policy `allowed` is a wei spend cap, not a gas limit. */
export const SESSION_MAX_GAS_WEI = 10_000_000_000_000_000n; // 0.01 ETH

export function isSupported7702Chain(chainId: number): boolean {
  return (Object.values(SUPPORTED_7702_CHAINS) as number[]).includes(chainId);
}

export function isMainnet7702Chain(chainId: number): boolean {
  return MAINNET_7702_CHAIN_IDS.includes(chainId);
}
