import { arbitrum, base, sepolia, type Chain } from "viem/chains";

/**
 * Chains where EIP-7702 is live and we have pinned Kernel v3.3 / EntryPoint 0.7
 * deployments (see services/integrations-ts/src/account-abstraction/pins.ts).
 * Mainnet execution remains behind DRY_RUN / ENABLE_MAINNET_TRANSFERS.
 */
export const SUPPORTED_7702_CHAINS: readonly Chain[] = [sepolia, base, arbitrum];

export function isSupported7702Chain(chainId: number): boolean {
  return SUPPORTED_7702_CHAINS.some((c) => c.id === chainId);
}
