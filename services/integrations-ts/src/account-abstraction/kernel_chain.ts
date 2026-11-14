/** Chain plumbing shared by the live Kernel grant backend and bundler. */
import { createPublicClient, getAddress, http, type Address, type Chain } from "viem";
import { arbitrum, base, mainnet, optimism } from "viem/chains";
import { parseDelegation } from "./delegation.js";
import { KERNEL_7702_DELEGATION_ADDRESS, SUPPORTED_7702_CHAINS } from "./pins.js";

export function chainOf(chainId: number): Chain {
  if (chainId === SUPPORTED_7702_CHAINS.ethereum) return mainnet;
  if (chainId === SUPPORTED_7702_CHAINS.optimism) return optimism;
  if (chainId === SUPPORTED_7702_CHAINS.base) return base;
  if (chainId === SUPPORTED_7702_CHAINS.arbitrum) return arbitrum;
  throw new Error(`unsupported_chain: ${chainId}`);
}

const env = (k: string, fallback: string) => process.env[k]?.trim() || fallback;

export function rpcUrlsFromEnv(): Record<number, string> {
  const { ethereum: e, optimism: o, base: b, arbitrum: a } = SUPPORTED_7702_CHAINS;
  return {
    [e]: env("RPC_URL_ETHEREUM", "https://ethereum-rpc.publicnode.com"),
    [o]: env("RPC_URL_OPTIMISM", "https://mainnet.optimism.io"),
    [b]: env("RPC_URL_BASE", "https://mainnet.base.org"),
    [a]: env("RPC_URL_ARBITRUM", "https://arb1.arbitrum.io/rpc"),
  };
}

export function bundlerUrlsFromEnv(): Record<number, string> {
  const { ethereum: e, optimism: o, base: b, arbitrum: a } = SUPPORTED_7702_CHAINS;
  return {
    [e]: env("BUNDLER_RPC_ETHEREUM", `https://public.pimlico.io/v2/${e}/rpc`),
    [o]: env("BUNDLER_RPC_OPTIMISM", `https://public.pimlico.io/v2/${o}/rpc`),
    [b]: env("BUNDLER_RPC_BASE", `https://public.pimlico.io/v2/${b}/rpc`),
    [a]: env("BUNDLER_RPC_ARBITRUM", `https://public.pimlico.io/v2/${a}/rpc`),
  };
}

export function publicClientFor(rpcUrls: Record<number, string>, chainId: number) {
  const url = rpcUrls[chainId];
  if (!url) throw new Error(`unsupported_chain: no RPC for ${chainId}`);
  return createPublicClient({ chain: chainOf(chainId), transport: http(url) });
}

/**
 * Without an existing delegation the SDK would sign and install one inside the
 * UserOp; delegation changes need their own owner approval, so refuse.
 */
export async function requireKernelDelegation(
  client: { getCode: (a: { address: Address }) => Promise<`0x${string}` | undefined> },
  owner: Address,
  chainId: number,
): Promise<void> {
  const delegation = parseDelegation(await client.getCode({ address: owner }));
  if (delegation.state !== "delegated" || delegation.target !== getAddress(KERNEL_7702_DELEGATION_ADDRESS)) {
    throw Object.assign(new Error(`not_delegated: owner has no Kernel 7702 delegation on chain ${chainId}`), {
      code: "not_delegated",
    });
  }
}
