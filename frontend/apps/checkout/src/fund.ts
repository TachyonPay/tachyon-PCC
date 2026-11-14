import { chainIdHex, encodeErc20Transfer, nativeBalance, tokenBalance } from "@pcc/wallet-onboarding";
import { connectWallet, type Eip1193 } from "./wallet";

function hexQuantity(value: bigint): `0x${string}` {
  return `0x${value.toString(16)}`;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** ETH reserved so Privy can send the Kernel type-4. L2s are cheap; Ethereum is not. */
export function gasReserveWei(chainId: number): bigint {
  return chainId === 1 ? 10n ** 15n : 2n * 10n ** 14n;
}

export { nativeBalance, tokenBalance };

export async function waitForFunds(opts: {
  chainId: number;
  token: string;
  account: string;
  usdc: bigint;
  eth: bigint;
  budgetMs?: number;
}): Promise<void> {
  const deadline = Date.now() + (opts.budgetMs ?? 90_000);
  for (;;) {
    const [usdc, eth] = await Promise.all([
      tokenBalance(opts.chainId, opts.token, opts.account),
      nativeBalance(opts.chainId, opts.account),
    ]);
    if (usdc >= opts.usdc && eth >= opts.eth) return;
    if (Date.now() >= deadline) throw new Error("Timed out waiting for funds to arrive on the checkout wallet.");
    await sleep(2000);
  }
}

/**
 * MetaMask (injected or WalletConnect) sends USDC and a little ETH to the Privy
 * checkout address. Kernel is not installed on MetaMask.
 */
export async function fundCheckoutWallet(opts: {
  onPairingUri: (uri: string) => void;
  chainId: number;
  token: string;
  to: `0x${string}`;
  usdc: bigint;
  eth: bigint;
}): Promise<void> {
  if (opts.usdc <= 0n && opts.eth <= 0n) return;
  const wallet: Eip1193 = await connectWallet(opts.onPairingUri);
  try {
    await wallet.request({ method: "wallet_switchEthereumChain", params: [{ chainId: chainIdHex(opts.chainId) }] });
  } catch {
    /* WalletConnect in an iframe often cannot switch; the send still carries the chain. */
  }
  const accounts = (await wallet.request({ method: "eth_requestAccounts" })) as string[];
  const from = accounts[0];
  if (!from) throw new Error("MetaMask returned no address.");
  if (from.toLowerCase() === opts.to.toLowerCase()) {
    throw new Error("Connect MetaMask, not the checkout wallet, to send funds.");
  }

  const beforeUsdc = await tokenBalance(opts.chainId, opts.token, opts.to);
  const beforeEth = await nativeBalance(opts.chainId, opts.to);

  if (opts.usdc > 0n) {
    const have = await tokenBalance(opts.chainId, opts.token, from);
    if (have < opts.usdc) throw new Error("MetaMask does not have enough USDC on this chain.");
    const tx = await wallet.request({
      method: "eth_sendTransaction",
      params: [
        {
          from,
          to: opts.token,
          data: encodeErc20Transfer(opts.to, opts.usdc),
          value: "0x0",
        },
      ],
    });
    if (typeof tx !== "string" || !tx.startsWith("0x")) throw new Error("MetaMask did not return a USDC transaction hash.");
  }

  if (opts.eth > 0n) {
    const have = await nativeBalance(opts.chainId, from);
    if (have < opts.eth) throw new Error("MetaMask does not have enough ETH for Kernel gas.");
    const tx = await wallet.request({
      method: "eth_sendTransaction",
      params: [{ from, to: opts.to, value: hexQuantity(opts.eth) }],
    });
    if (typeof tx !== "string" || !tx.startsWith("0x")) throw new Error("MetaMask did not return an ETH transaction hash.");
  }

  await waitForFunds({
    chainId: opts.chainId,
    token: opts.token,
    account: opts.to,
    usdc: beforeUsdc + opts.usdc,
    eth: beforeEth + opts.eth,
  });
}
