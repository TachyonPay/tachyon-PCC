/** Public JSON-RPC reads when the connected wallet cannot eth_getCode / eth_call. */

export const PUBLIC_RPC: Record<number, string> = {
  1: "https://ethereum-rpc.publicnode.com",
  10: "https://mainnet.optimism.io",
  8453: "https://mainnet.base.org",
  42161: "https://arb1.arbitrum.io/rpc",
};

export async function publicRpc(chainId: number, method: string, params: unknown[]): Promise<unknown> {
  const url = PUBLIC_RPC[chainId];
  if (!url) throw new Error(`no public RPC for chain ${chainId}`);
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  const body = (await res.json()) as { result?: unknown; error?: { message?: string } };
  if (body.error) throw new Error(body.error.message ?? `${method} failed`);
  return body.result;
}

const BALANCE_OF = "0x70a08231";

function addressWord(account: string): string {
  return account.slice(2).toLowerCase().padStart(64, "0");
}

export async function nativeBalance(chainId: number, account: string): Promise<bigint> {
  const result = await publicRpc(chainId, "eth_getBalance", [account, "latest"]);
  if (typeof result !== "string" || !/^0x[0-9a-fA-F]+$/.test(result)) throw new Error("eth_getBalance returned no balance");
  return BigInt(result);
}

export async function tokenBalance(chainId: number, token: string, account: string): Promise<bigint> {
  const result = await publicRpc(chainId, "eth_call", [
    { to: token, data: `${BALANCE_OF}${addressWord(account)}` },
    "latest",
  ]);
  if (typeof result !== "string" || !/^0x[0-9a-fA-F]*$/.test(result)) throw new Error("eth_call returned no balance");
  return BigInt(result === "0x" ? "0x0" : result);
}
