/**
 * Sidekey-style Kernel install: Privy signs only the 7702 authorization.
 * The bundler carries it on the first UserOp. No type-4 from the embedded wallet.
 */
import { createKernelAccount, createKernelAccountClient } from "@zerodev/sdk";
import { KERNEL_V3_3, getEntryPoint } from "@zerodev/sdk/constants";
import {
  KERNEL_7702_IMPLEMENTATION,
  PUBLIC_RPC,
  chainIdHex,
  installPlan,
} from "@pcc/wallet-onboarding";
import { createPublicClient, createWalletClient, custom, http, type Address, type Hex } from "viem";
import { toAccount, type LocalAccount, type SignAuthorizationReturnType } from "viem/accounts";
import { arbitrum, base, mainnet, optimism, type Chain } from "viem/chains";
import type { Eip1193 } from "./wallet";

const ENTRY_POINT = getEntryPoint("0.7");
const KERNEL_VERSION = KERNEL_V3_3;

const CHAINS: Record<number, Chain> = {
  1: mainnet,
  10: optimism,
  8453: base,
  42161: arbitrum,
};

function bundlerUrl(chainId: number): string {
  return `https://public.pimlico.io/v2/${chainId}/rpc`;
}

async function privySigner(provider: Eip1193, address: Address, chain: Chain): Promise<LocalAccount> {
  const wc = createWalletClient({ account: address, chain, transport: custom(provider) });
  return toAccount({
    address,
    signMessage: ({ message }) => wc.signMessage({ message }),
    signTypedData: (typedData) => wc.signTypedData(typedData as never),
    signTransaction: async () => {
      throw new Error("the Tachyon wallet does not sign raw transactions");
    },
  });
}

export async function installKernelViaUserOp(opts: {
  provider: Eip1193;
  address: Address;
  chainId: number;
  signAuth: () => Promise<SignAuthorizationReturnType>;
}): Promise<Hex | null> {
  const chain = CHAINS[opts.chainId];
  const rpc = PUBLIC_RPC[opts.chainId];
  if (!chain || !rpc) throw new Error(`unsupported chain ${opts.chainId}`);
  const publicClient = createPublicClient({ chain, transport: http(rpc) });
  const first = installPlan((await publicClient.getCode({ address: opts.address })) ?? "0x", KERNEL_7702_IMPLEMENTATION);
  if (first.action === "skip") return null;
  if (first.action === "refuse") throw new Error(first.reason);

  try {
    await opts.provider.request({ method: "wallet_switchEthereumChain", params: [{ chainId: chainIdHex(opts.chainId) }] });
  } catch {
    /* Privy may already be on this chain; the 7702 auth still carries chainId. */
  }

  const signer = await privySigner(opts.provider, opts.address, chain);
  const eip7702Auth = await opts.signAuth();
  const account = await createKernelAccount(publicClient, {
    eip7702Account: signer,
    eip7702Auth,
    entryPoint: ENTRY_POINT,
    kernelVersion: KERNEL_VERSION,
  });
  if (account.address.toLowerCase() !== opts.address.toLowerCase()) {
    throw new Error("Kernel account address must equal the Tachyon wallet");
  }

  const fees = async () => {
    const bundler = bundlerUrl(opts.chainId);
    const f = await publicClient.estimateFeesPerGas();
    const minTip = 2_000_000n;
    let tip = f.maxPriorityFeePerGas ?? minTip;
    if (tip < minTip) tip = minTip;
    let max = (f.maxFeePerGas ?? tip) * 2n;
    if (max < tip) max = tip;
    try {
      const res = await fetch(bundler, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "pimlico_getUserOperationGasPrice", params: [] }),
      });
      const json = (await res.json()) as {
        result?: { fast?: { maxFeePerGas: string; maxPriorityFeePerGas: string } };
      };
      const fast = json.result?.fast;
      if (fast) {
        const pTip = BigInt(fast.maxPriorityFeePerGas);
        const pMax = BigInt(fast.maxFeePerGas);
        if (pTip > tip) tip = pTip;
        if (pMax > max) max = pMax;
      }
    } catch {
      /* public estimate plus the floor is enough if Pimlico gas-price is down */
    }
    return { maxFeePerGas: max, maxPriorityFeePerGas: tip };
  };
  const client = createKernelAccountClient({
    account,
    chain,
    bundlerTransport: http(bundlerUrl(opts.chainId)),
    client: publicClient,
    userOperation: { estimateFeesPerGas: fees },
  });
  const userOpHash = await client.sendUserOperation({
    callData: await account.encodeCalls([{ to: opts.address, value: 0n, data: "0x" }]),
  });
  const rcpt = await client.waitForUserOperationReceipt({ hash: userOpHash, timeout: 180_000 });
  if (!rcpt.success) throw new Error("Kernel install UserOp failed");
  const after = installPlan((await publicClient.getCode({ address: opts.address })) ?? "0x", KERNEL_7702_IMPLEMENTATION);
  if (after.action !== "skip") throw new Error("bundler sent a transaction that did not install Kernel");
  return rcpt.receipt.transactionHash;
}
