/**
 * Browser-wallet helpers for the hosted checkout: which connector to use and
 * how to hand the session-grant payload to `eth_signTypedData_v4`.
 */

export type WalletSource = "injected" | "walletconnect" | "none";

/** Prefer an injected wallet; fall back to WalletConnect only when a project id is configured. */
export function walletSource(opts: { hasInjected: boolean; walletConnectProjectId?: string | undefined }): WalletSource {
  if (opts.hasInjected) return "injected";
  if (opts.walletConnectProjectId && /^[0-9a-f]{32}$/i.test(opts.walletConnectProjectId.trim())) return "walletconnect";
  return "none";
}

export function chainIdHex(chainId: number): `0x${string}` {
  if (!Number.isInteger(chainId) || chainId <= 0) throw new Error("chainId must be a positive integer");
  return `0x${chainId.toString(16)}`;
}

const DOMAIN_FIELDS = [
  ["name", "string"],
  ["version", "string"],
  ["chainId", "uint256"],
  ["verifyingContract", "address"],
  ["salt", "bytes32"],
] as const;

interface TypedDataJson {
  domain: Record<string, unknown>;
  types: Record<string, { name: string; type: string }[]>;
  primaryType: string;
  message: Record<string, unknown>;
}

/**
 * Serialize for `eth_signTypedData_v4`. Wallets require an explicit
 * `EIP712Domain` type listing exactly the domain fields present, in the
 * canonical order; everything else passes through untouched.
 */
export function toSignTypedDataV4(typedData: unknown): string {
  const td = typedData as TypedDataJson;
  if (!td || typeof td !== "object" || !td.domain || !td.types || !td.primaryType || !td.message) {
    throw new Error("typed data must have domain, types, primaryType and message");
  }
  const EIP712Domain = DOMAIN_FIELDS.filter(([k]) => td.domain[k] !== undefined).map(([name, type]) => ({ name, type }));
  return JSON.stringify({ ...td, types: { EIP712Domain, ...td.types } });
}
