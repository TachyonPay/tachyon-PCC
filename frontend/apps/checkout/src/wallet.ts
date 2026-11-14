import { walletSource } from "@pcc/wallet-onboarding";

export type Eip1193 = {
  request: (args: { method: string; params?: unknown[] }) => Promise<unknown>;
};

const WALLETCONNECT_PROJECT_ID = process.env["NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID"];
const SOURCE_CHAINS = [1, 10, 8453, 42161] as const;
const LOCAL_SIGNER = "http://127.0.0.1:1790";

function injected(): Eip1193 | undefined {
  if (typeof window === "undefined") return undefined;
  return (window as unknown as { ethereum?: Eip1193 }).ethereum;
}

function loopbackHost(): boolean {
  if (typeof window === "undefined") return false;
  const host = window.location.hostname;
  return host === "127.0.0.1" || host === "localhost";
}

/** Wallet A loopback signer (scripts/local-injected-wallet.ts). Signatures only. */
async function localSigner(): Promise<Eip1193 | undefined> {
  if (!loopbackHost()) return undefined;
  try {
    const health = await fetch(`${LOCAL_SIGNER}/health`, { signal: AbortSignal.timeout(400) });
    if (!health.ok) return undefined;
  } catch {
    return undefined;
  }
  return {
    request: async ({ method, params }) => {
      const res = await fetch(`${LOCAL_SIGNER}/rpc`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: params ?? [] }),
      });
      const body = (await res.json()) as { result?: unknown; error?: { message?: string } };
      if (body.error) throw new Error(body.error.message ?? "local wallet rpc failed");
      return body.result;
    },
  };
}

/**
 * Resolve a wallet provider. WalletConnect is loaded lazily and only when no
 * injected wallet exists; `onPairingUri` receives the `wc:` URI to show the payer.
 * On loopback, a running local Wallet A signer is preferred so UI tests skip QR.
 */
export async function connectWallet(onPairingUri: (uri: string) => void): Promise<Eip1193> {
  const local = await localSigner();
  if (local) return local;
  const kind = walletSource({ hasInjected: injected() !== undefined, walletConnectProjectId: WALLETCONNECT_PROJECT_ID });
  if (kind === "injected") return injected()!;
  if (kind === "none") {
    throw new Error("No wallet found. Install a browser wallet, or set NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID for WalletConnect.");
  }
  const { EthereumProvider } = await import("@walletconnect/ethereum-provider");
  const wc = await EthereumProvider.init({
    projectId: WALLETCONNECT_PROJECT_ID!.trim(),
    chains: [SOURCE_CHAINS[2]],
    optionalChains: [...SOURCE_CHAINS],
    showQrModal: true,
    metadata: {
      name: "Tachyon Pay",
      description: "Private crypto checkout",
      url: "https://pay.tachyon.pe",
      icons: ["https://pay.tachyon.pe/logo.svg"],
    },
    methods: ["personal_sign", "eth_signTypedData_v4", "eth_sendTransaction", "wallet_switchEthereumChain"],
    optionalMethods: [
      "eth_signAuthorization",
      "wallet_signAuthorization",
      "eth_getCode",
      "eth_getTransactionCount",
    ],
  });
  wc.on("display_uri", onPairingUri);
  await wc.connect();
  return wc as unknown as Eip1193;
}
