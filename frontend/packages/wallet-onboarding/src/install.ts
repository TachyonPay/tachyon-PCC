/**
 * Ask the connected wallet to install the Kernel v3.3 EIP-7702 delegation.
 * This module holds no key. It refuses when the account already has code,
 * including a delegation to any other implementation.
 */
import { type Address, type Hex, getAddress } from "viem";
import { onboardDecision, parseDelegation } from "./delegation";
import { publicRpc } from "./rpc";

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

/** WalletConnect often returns a number, or nothing, instead of a 0x quantity. */
export function parseRpcQuantity(value: unknown): bigint | null {
  if (typeof value === "bigint" && value >= 0n) return value;
  if (typeof value === "number" && Number.isInteger(value) && value >= 0) return BigInt(value);
  if (typeof value === "string" && /^(0x[0-9a-fA-F]+|[0-9]+)$/.test(value.trim())) return BigInt(value.trim());
  return null;
}

function asRecord(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== "object") throw new Error(`${label} was not returned by the wallet`);
  return value as Record<string, unknown>;
}

async function accountCode(provider: Eip1193Provider, chainId: number, account: Address): Promise<Hex> {
  try {
    const code = await provider.request({ method: "eth_getCode", params: [account, "latest"] });
    if (typeof code === "string") return code as Hex;
  } catch {
    /* WalletConnect often cannot eth_getCode; fall through to public RPC. */
  }
  const code = await publicRpc(chainId, "eth_getCode", [account, "latest"]);
  if (typeof code !== "string") throw new Error("eth_getCode returned no code");
  return code as Hex;
}

async function accountNonce(provider: Eip1193Provider, chainId: number, account: Address): Promise<bigint> {
  try {
    const n = parseRpcQuantity(
      await provider.request({ method: "eth_getTransactionCount", params: [account, "pending"] }),
    );
    if (n !== null) return n;
  } catch {
    /* WalletConnect often cannot eth_getTransactionCount. */
  }
  const n = parseRpcQuantity(await publicRpc(chainId, "eth_getTransactionCount", [account, "pending"]));
  if (n === null) throw new Error("could not read account nonce");
  return n;
}

export const KERNEL_INSTALL_UNSUPPORTED =
  "This wallet cannot install Kernel. MetaMask and most extensions refuse EIP-7702 from a website. Connect a wallet that can sign EIP-7702 for this install, then Sign allowance here.";

export interface Signed7702Authorization {
  r: string;
  s: string;
  yParity?: string | number;
  v?: string | number;
  address?: string;
  contractAddress?: string;
}

export type AuthorizationSource =
  | Signed7702Authorization
  | ((ctx: { chainId: number; nonce: bigint }) => Promise<Signed7702Authorization>);

export type Type4SendRequest = {
  from: Address;
  chainId: number;
  kernel: Address;
  nonce: bigint;
  signed: { r: string; s: string; yParity: string | number };
};

export interface InstallOptions {
  /** How long to wait for Kernel bytecode after the type-4 send. */
  confirmMs?: number;
  /** Pre-signed 7702 auth (Privy `useSign7702Authorization`). */
  authorization?: AuthorizationSource;
  /** Privy `useSendTransaction`. The EIP-1193 provider drops `authorizationList`. */
  send?: (tx: Type4SendRequest) => Promise<Hex>;
}

function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

function isMissingRpcMethod(message: string): boolean {
  return /method|not support|unsupported|does not exist|not found|not available/i.test(message);
}

function isWallet7702Blocked(message: string): boolean {
  return /eip-7702|authorizationlist|type.?4|external eip-7702/i.test(message);
}

function yParityOf(signed: Record<string, unknown>): string | number | undefined {
  const y = signed["yParity"] ?? signed["v"];
  if (typeof y === "bigint") {
    const n = Number(y);
    if (n === 27 || n === 28) return n - 27;
    return n;
  }
  if (typeof y === "number") {
    if (y === 27 || y === 28) return y - 27;
    return y;
  }
  if (typeof y === "string") return y;
  return undefined;
}

/** Privy type-4 RPCs want 0 or 1, not 27/28 or a hex string. */
export function yParityBit(value: string | number): 0 | 1 {
  if (typeof value === "number") {
    if (value === 27 || value === 28) return (value - 27) as 0 | 1;
    return (value & 1) as 0 | 1;
  }
  const n = BigInt(value);
  if (n === 27n || n === 28n) return Number(n - 27n) as 0 | 1;
  return Number(n & 1n) as 0 | 1;
}

async function resolveAuthorization(
  options: InstallOptions,
  provider: Eip1193Provider,
  kernel: Address,
  chainId: number,
  nonce: bigint,
): Promise<Record<string, unknown> | null> {
  if (!options.authorization) return signAuthorization(provider, kernel, chainId, nonce);
  const value =
    typeof options.authorization === "function"
      ? await options.authorization({ chainId, nonce })
      : options.authorization;
  return value as unknown as Record<string, unknown>;
}

async function signAuthorization(
  provider: Eip1193Provider,
  implementation: Address,
  chainId: number,
  nonce: bigint,
): Promise<Record<string, unknown> | null> {
  const params = [{ chainId: hexQuantity(chainId), address: implementation, nonce: hexQuantity(nonce) }];
  let last = "";
  for (const method of ["eth_sign7702Authorization", "eth_signAuthorization", "wallet_signAuthorization"]) {
    try {
      return asRecord(await provider.request({ method, params }), "authorization");
    } catch (e) {
      last = errorText(e);
      if (!isMissingRpcMethod(last)) throw e;
    }
  }
  return null;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForKernel(
  provider: Eip1193Provider,
  chainId: number,
  owner: Address,
  kernel: Address,
  budgetMs: number,
): Promise<void> {
  const deadline = Date.now() + Math.max(0, budgetMs);
  for (;;) {
    const plan = installPlan(await accountCode(provider, chainId, owner), kernel);
    if (plan.action === "skip") return;
    if (plan.action === "refuse") throw new Error("account delegated to a different implementation after send");
    if (Date.now() >= deadline) break;
    await sleep(Math.min(1500, Math.max(0, deadline - Date.now())));
  }
  throw new Error("wallet sent a transaction that did not install Kernel");
}

async function sendType4(
  provider: Eip1193Provider,
  owner: Address,
  chainId: number,
  kernel: Address,
  nonce: bigint,
  signed?: { r: string; s: string; yParity: string | number },
): Promise<Hex> {
  const authorization: Record<string, unknown> = {
    chainId: hexQuantity(chainId),
    address: kernel,
    nonce: hexQuantity(nonce),
  };
  const txRequest: Record<string, unknown> = {
    from: owner,
    to: owner,
    chainId,
    value: "0x0",
    data: "0x",
    type: "0x04",
    authorizationList: [authorization],
  };
  if (signed) {
    const yParity = yParityBit(signed.yParity);
    authorization["yParity"] = yParity;
    authorization["r"] = signed.r;
    authorization["s"] = signed.s;
    // Privy embedded wallets read snake_case `authorization_list` and drop camelCase.
    txRequest["type"] = 4;
    txRequest["authorization_list"] = [
      {
        contract: kernel,
        address: kernel,
        chain_id: chainId,
        chainId,
        nonce: Number(nonce),
        r: signed.r,
        s: signed.s,
        y_parity: yParity,
        yParity,
      },
    ];
  }
  const tx = await provider.request({
    method: "eth_sendTransaction",
    params: [txRequest],
  });
  if (typeof tx !== "string" || !tx.startsWith("0x")) throw new Error("wallet did not return a transaction hash");
  return tx as Hex;
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
  options: InstallOptions = {},
): Promise<Hex | null> {
  const owner = getAddress(account);
  const kernel = getAddress(implementation);
  const confirmMs = options.confirmMs ?? 45_000;
  try {
    await provider.request({ method: "wallet_switchEthereumChain", params: [{ chainId: hexQuantity(chainId) }] });
  } catch {
    /* WalletConnect + MetaMask in an iframe often cannot switch; the 7702 auth still carries chainId. */
  }

  const first = installPlan(await accountCode(provider, chainId, owner), kernel);
  if (first.action === "skip") return null;
  if (first.action === "refuse") throw new Error(first.reason);

  const nonce = await accountNonce(provider, chainId, owner);

  const beforeSign = installPlan(await accountCode(provider, chainId, owner), kernel);
  if (beforeSign.action !== "install") throw new Error("account code changed before signing; delegation was not installed");

  const signed = await resolveAuthorization(options, provider, kernel, chainId, nonce);
  let typed: { r: string; s: string; yParity: string | number } | undefined;
  if (signed) {
    const authorized = String(signed["address"] ?? signed["contractAddress"] ?? kernel).toLowerCase();
    if (authorized !== kernel.toLowerCase()) throw new Error("wallet authorized a different contract; delegation was not installed");
    const r = signed["r"];
    const s = signed["s"];
    const yParity = yParityOf(signed);
    if (typeof r !== "string" || typeof s !== "string" || (typeof yParity !== "string" && typeof yParity !== "number")) {
      throw new Error("wallet returned an incomplete authorization");
    }
    typed = { r, s, yParity };
  }

  const beforeSend = installPlan(await accountCode(provider, chainId, owner), kernel);
  if (beforeSend.action !== "install") throw new Error("account code changed before sending; delegation was not installed");

  try {
    const tx =
      options.send && typed
        ? await options.send({ from: owner, chainId, kernel, nonce, signed: typed })
        : await sendType4(provider, owner, chainId, kernel, nonce, typed);
    await waitForKernel(provider, chainId, owner, kernel, confirmMs);
    return tx;
  } catch (e) {
    const message = errorText(e);
    if (!typed && (isMissingRpcMethod(message) || isWallet7702Blocked(message))) {
      throw new Error(KERNEL_INSTALL_UNSUPPORTED);
    }
    throw e;
  }
}
