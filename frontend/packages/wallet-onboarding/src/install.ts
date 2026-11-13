/**
 * Ask the connected wallet to install the Kernel v3.3 EIP-7702 delegation.
 * This module holds no key. It refuses when the account already has code,
 * including a delegation to any other implementation.
 */
import { type Address, type Hex, getAddress } from "viem";
import { onboardDecision, parseDelegation } from "./delegation";

export const KERNEL_7702_IMPLEMENTATION = "0xd6CEDDe84be40893d153Be9d467CD6aD37875b28" as const;

export interface Eip1193Provider {
  request(args: { method: string; params?: unknown[] }): Promise<unknown>;
}

export type InstallPlan = { action: "install" } | { action: "skip" } | { action: "refuse"; reason: string };

/** What the page may do with the account code it just read. */
export function installPlan(code: Hex | undefined | null, implementation: Address): InstallPlan {
  const decision = onboardDecision(parseDelegation(code), [implementation]);
  if (decision.action === "offer_delegation") return { action: "install" };
  if (decision.action === "already_ours") return { action: "skip" };
  return { action: "refuse", reason: decision.reason };
}

function hexQuantity(value: bigint | number): Hex {
  return `0x${BigInt(value).toString(16)}`;
}

function asRecord(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== "object") throw new Error(`${label} was not returned by the wallet`);
  return value as Record<string, unknown>;
}

async function accountCode(provider: Eip1193Provider, account: Address): Promise<Hex> {
  const code = await provider.request({ method: "eth_getCode", params: [account, "latest"] });
  if (typeof code !== "string") throw new Error("eth_getCode returned no code");
  return code as Hex;
}

function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

async function signAuthorization(
  provider: Eip1193Provider,
  implementation: Address,
  chainId: number,
  nonce: bigint,
): Promise<Record<string, unknown>> {
  const params = [{ chainId: hexQuantity(chainId), address: implementation, nonce: hexQuantity(nonce) }];
  let last = "This wallet cannot sign an EIP-7702 authorization.";
  for (const method of ["eth_signAuthorization", "wallet_signAuthorization"]) {
    try {
      return asRecord(await provider.request({ method, params }), "authorization");
    } catch (e) {
      last = errorText(e);
      if (!/method|not support|unsupported|does not exist|not found/i.test(last)) throw e;
    }
  }
  throw new Error(last);
}

/**
 * Install Kernel on `chainId` for `account`. Returns the delegation transaction
 * hash, or null when the account already delegates to Kernel.
 */
export async function installKernelDelegation(
  provider: Eip1193Provider,
  account: Address,
  chainId: number,
  implementation: Address = KERNEL_7702_IMPLEMENTATION,
): Promise<Hex | null> {
  const owner = getAddress(account);
  const kernel = getAddress(implementation);
  await provider.request({ method: "wallet_switchEthereumChain", params: [{ chainId: hexQuantity(chainId) }] });

  const first = installPlan(await accountCode(provider, owner), kernel);
  if (first.action === "skip") return null;
  if (first.action === "refuse") throw new Error(first.reason);

  const nonceHex = await provider.request({ method: "eth_getTransactionCount", params: [owner, "pending"] });
  if (typeof nonceHex !== "string") throw new Error("wallet returned no nonce");
  const nonce = BigInt(nonceHex);

  const beforeSign = installPlan(await accountCode(provider, owner), kernel);
  if (beforeSign.action !== "install") throw new Error("account code changed before signing; delegation was not installed");

  const signed = await signAuthorization(provider, kernel, chainId, nonce);
  const authorized = String(signed["address"] ?? signed["contractAddress"] ?? "").toLowerCase();
  if (authorized !== kernel.toLowerCase()) throw new Error("wallet authorized a different contract; delegation was not installed");
  const r = signed["r"];
  const s = signed["s"];
  const yParity = signed["yParity"] ?? signed["v"];
  if (typeof r !== "string" || typeof s !== "string" || (typeof yParity !== "string" && typeof yParity !== "number")) {
    throw new Error("wallet returned an incomplete authorization");
  }

  const beforeSend = installPlan(await accountCode(provider, owner), kernel);
  if (beforeSend.action !== "install") throw new Error("account code changed before sending; delegation was not installed");

  const tx = await provider.request({
    method: "eth_sendTransaction",
    params: [
      {
        from: owner,
        to: owner,
        value: "0x0",
        data: "0x",
        type: "0x04",
        authorizationList: [{ chainId: hexQuantity(chainId), address: kernel, nonce: hexQuantity(nonce), yParity, r, s }],
      },
    ],
  });
  if (typeof tx !== "string" || !tx.startsWith("0x")) throw new Error("wallet did not return a transaction hash");
  return tx as Hex;
}
