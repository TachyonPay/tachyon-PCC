/**
 * Pinned account stack for the EIP-7702 path. One Kernel version, one
 * EntryPoint, one SDK major. Do not mix with Kernel v4 docs (EntryPoint v0.9)
 * which the kernel repo `dev` README currently describes.
 */
import { KERNEL_V3_3, getEntryPoint } from "@zerodev/sdk/constants";

export const KERNEL_VERSION = KERNEL_V3_3;
export const ENTRY_POINT = getEntryPoint("0.7");

/** Chains where ZeroDev documents 7702 support and we have reviewed RPCs. */
export const SUPPORTED_7702_CHAINS = {
  sepolia: 11155111,
  base: 8453,
  arbitrum: 42161,
} as const;
