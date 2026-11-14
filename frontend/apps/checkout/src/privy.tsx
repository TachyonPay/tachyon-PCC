"use client";

import {
  PrivyProvider,
  getEmbeddedConnectedWallet,
  useCreateWallet,
  usePrivy,
  useSign7702Authorization,
  useWallets,
} from "@privy-io/react-auth";
import { KERNEL_7702_IMPLEMENTATION, SUPPORTED_7702_CHAINS, yParityBit } from "@pcc/wallet-onboarding";
import { type ReactNode, useCallback } from "react";
import type { SignAuthorizationReturnType } from "viem/accounts";
import type { Eip1193 } from "./wallet";

export const PRIVY_APP_ID = (process.env["NEXT_PUBLIC_PRIVY_APP_ID"] ?? "").trim();

const PAY_IN_CHAIN_IDS = new Set([1, 10, 8453, 42161]);
const PAY_IN_CHAINS = SUPPORTED_7702_CHAINS.filter((c) => PAY_IN_CHAIN_IDS.has(c.id));

export function privyConfigured(): boolean {
  return /^[a-z0-9]+$/i.test(PRIVY_APP_ID) && PRIVY_APP_ID.length >= 16;
}

export function PrivyRoot({ children }: { children: ReactNode }) {
  if (!privyConfigured()) return children;
  const defaultChain = PAY_IN_CHAINS.find((c) => c.id === 1) ?? PAY_IN_CHAINS[0];
  return (
    <PrivyProvider
      appId={PRIVY_APP_ID}
      config={{
        loginMethods: ["email", "google"],
        embeddedWallets: {
          ethereum: { createOnLogin: "users-without-wallets" },
          showWalletUIs: false,
        },
        appearance: {
          theme: "dark",
          landingHeader: "Tachyon checkout wallet",
          loginMessage: "This wallet signs Kernel, the allowance, and TOTP stays yours.",
        },
        ...(defaultChain ? { defaultChain: defaultChain as never } : {}),
        supportedChains: PAY_IN_CHAINS as never,
        legal: {
          termsAndConditionsUrl: "https://tachyon.pe",
          privacyPolicyUrl: "https://tachyon.pe",
        },
      }}
    >
      {children}
    </PrivyProvider>
  );
}

export type PrivyPayer = {
  configured: true;
  ready: boolean;
  authenticated: boolean;
  login: () => void;
  logout: () => Promise<void>;
  provider: () => Promise<Eip1193>;
  signKernelAuth: (chainId: number) => Promise<SignAuthorizationReturnType>;
};

export function usePrivyPayer(): PrivyPayer {
  const privy = usePrivy();
  const { wallets } = useWallets();
  const { createWallet } = useCreateWallet();
  const { signAuthorization } = useSign7702Authorization();
  const embedded = getEmbeddedConnectedWallet(wallets);

  const provider = useCallback(async (): Promise<Eip1193> => {
    let wallet = embedded ?? getEmbeddedConnectedWallet(wallets);
    if (!wallet) {
      try {
        const created = await createWallet();
        const raw = await (created as { getEthereumProvider?: () => Promise<unknown> }).getEthereumProvider?.();
        if (raw) return raw as Eip1193;
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        if (!/already has an embedded wallet/i.test(msg)) throw e;
      }
      wallet = getEmbeddedConnectedWallet(wallets);
    }
    if (!wallet) throw new Error("Privy embedded wallet is not ready.");
    return (await wallet.getEthereumProvider()) as Eip1193;
  }, [createWallet, embedded, wallets]);

  const signKernelAuth = useCallback(
    async (chainId: number): Promise<SignAuthorizationReturnType> => {
      const address = embedded?.address;
      if (!address) throw new Error("Privy embedded wallet is not ready.");
      const a = await signAuthorization(
        { contractAddress: KERNEL_7702_IMPLEMENTATION, chainId },
        { address },
      );
      const yParity = yParityBit((a.yParity ?? a.v ?? 0) as string | number);
      return {
        address: KERNEL_7702_IMPLEMENTATION,
        chainId: Number(a.chainId ?? chainId),
        nonce: Number(a.nonce),
        r: a.r,
        s: a.s,
        yParity,
        v: a.v !== undefined ? BigInt(a.v) : undefined,
      };
    },
    [embedded, signAuthorization],
  );

  return {
    configured: true,
    ready: privy.ready,
    authenticated: privy.authenticated,
    login: () => privy.login(),
    logout: privy.logout,
    provider,
    signKernelAuth,
  };
}
