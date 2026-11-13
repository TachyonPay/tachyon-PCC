/**
 * Session-key UserOp admission. Every out-of-policy shape the M3 tests name
 * is rejected here *before* a bundler is contacted.
 */
import { getAddress, isAddress, type Address, type Hex } from "viem";
import { decodeErc20Transfer, selectorOf, APPROVE_SELECTOR, CallError } from "./call.js";
import { checkBroadcast, type ExecutionGates } from "./gates.js";
import { onboardDecision, parseDelegation, type OnboardDecision } from "./delegation.js";
import {
  KERNEL_7702_DELEGATION_ADDRESS,
  SESSION_MAX_GAS,
  isSupported7702Chain,
} from "./pins.js";

export type CallKind = "call" | "delegatecall";

export interface SessionGrant {
  owner: Address;
  sessionSigner: Address;
  chainId: number;
  token: Address;
  maxAmount: bigint;
  validUntil: number;
  revoked: boolean;
  kernelImplementation: Address;
  maxGas: bigint;
  maxGasWei: bigint;
  rateLimitCount: number;
  /** Omit for POC option C (any recipient). */
  recipient?: Address;
}

export interface SessionCall {
  chainId: number;
  from: Address;
  to: Address;
  data: Hex;
  value: bigint;
  callKind: CallKind;
  gasLimit: bigint;
  approvalRef: string;
  attemptId: string;
  /**
   * 1Click deposit address from the verified quote. When set, the ERC-20
   * transfer recipient must equal this address (quote-address substitution).
   */
  quoteDepositAddress?: Address;
  /** Autopay mandate whose allowance signs this call. */
  mandateId?: string;
}

export interface Eip7702Auth {
  chainId: number;
  address: Address;
  nonce: bigint;
  signer: Address;
}

export type SessionRejectCode =
  | "uninitialized"
  | "no_session"
  | "revoked"
  | "expired"
  | "wrong_chain"
  | "unsupported_chain"
  | "owner_mismatch"
  | "delegatecall"
  | "native_value"
  | "approve"
  | "malicious_selector"
  | "arbitrary_target"
  | "excessive_amount"
  | "recipient_mismatch"
  | "excessive_gas"
  | "invalid_owner_auth"
  | "auth_wrong_chain"
  | "auth_wrong_implementation"
  | "auth_replay"
  | "already_delegated"
  | "not_eoa"
  | "dry_run"
  | "mainnet_disabled"
  | "missing_approval"
  | "quote_address_mismatch"
  | "amount_mismatch"
  | "signer_disabled"
  | "chain_not_enrolled";

export type SessionDecision = { ok: true } | { ok: false; code: SessionRejectCode; reason: string };

export function validateOnboard(
  code: Hex | undefined | null,
  desired: Address = KERNEL_7702_DELEGATION_ADDRESS,
): { decision: OnboardDecision; parsed: ReturnType<typeof parseDelegation> } {
  const parsed = parseDelegation(code);
  return { parsed, decision: onboardDecision(parsed, desired, [KERNEL_7702_DELEGATION_ADDRESS]) };
}

export function assertOnboardAllowed(code: Hex | undefined | null): SessionDecision {
  const { decision } = validateOnboard(code);
  switch (decision.decision) {
    case "install":
    case "already_compatible":
      return { ok: true };
    case "requires_owner_review":
      return {
        ok: false,
        code: "already_delegated",
        reason: `EOA already delegates to unfamiliar implementation ${decision.current}`,
      };
    case "refuse":
      return { ok: false, code: "not_eoa", reason: "address is a contract account, not an EOA" };
  }
}

export function validateAuthorization(
  auth: Eip7702Auth,
  expected: { owner: Address; chainId: number; currentNonce: bigint; implementation?: Address },
): SessionDecision {
  const impl = expected.implementation ?? KERNEL_7702_DELEGATION_ADDRESS;
  if (!isAddress(auth.signer) || getAddress(auth.signer) !== getAddress(expected.owner)) {
    return { ok: false, code: "invalid_owner_auth", reason: "7702 authorization signer is not the owner" };
  }
  if (auth.chainId !== expected.chainId) {
    return { ok: false, code: "auth_wrong_chain", reason: `7702 authorization chain ${auth.chainId} != ${expected.chainId}` };
  }
  if (getAddress(auth.address) !== getAddress(impl)) {
    return { ok: false, code: "auth_wrong_implementation", reason: "7702 authorization targets the wrong implementation" };
  }
  if (auth.nonce !== expected.currentNonce) {
    return { ok: false, code: "auth_replay", reason: "7702 authorization nonce does not match the account nonce" };
  }
  return { ok: true };
}

export function validateSessionCall(
  call: SessionCall,
  grant: SessionGrant | null | undefined,
  nowUnix: number,
  gates?: ExecutionGates,
  mode: "simulate" | "execute" = "simulate",
): SessionDecision {
  if (mode === "execute") {
    const g = gates ?? { dryRun: true, enableMainnetTransfers: false };
    const gate = checkBroadcast(g, call.chainId, call.approvalRef);
    if (!gate.ok) {
      return { ok: false, code: gate.reason, reason: gate.reason };
    }
  }

  if (!grant) return { ok: false, code: "no_session", reason: "no session permission granted" };
  if (grant.revoked) return { ok: false, code: "revoked", reason: "session has been revoked by the owner" };
  if (!Number.isInteger(grant.validUntil) || grant.validUntil <= nowUnix) {
    return { ok: false, code: "expired", reason: "session has expired" };
  }
  if (!isSupported7702Chain(call.chainId) || call.chainId !== grant.chainId) {
    return {
      ok: false,
      code: isSupported7702Chain(call.chainId) ? "wrong_chain" : "unsupported_chain",
      reason: `chain ${call.chainId} is not the enrolled session chain ${grant.chainId}`,
    };
  }
  if (getAddress(call.from) !== getAddress(grant.owner)) {
    return { ok: false, code: "owner_mismatch", reason: "UserOp sender is not the enrolled owner" };
  }
  if (call.callKind === "delegatecall") {
    return { ok: false, code: "delegatecall", reason: "refusing delegatecall UserOp" };
  }
  if (call.value !== 0n) {
    return { ok: false, code: "native_value", reason: "session key cannot send native value" };
  }
  if (getAddress(call.to) !== getAddress(grant.token)) {
    return { ok: false, code: "arbitrary_target", reason: "call target is not the allowlisted token" };
  }

  let decoded: { to: Address; amount: bigint };
  try {
    if (selectorOf(call.data) === APPROVE_SELECTOR) {
      return { ok: false, code: "approve", reason: "refusing ERC-20 approve" };
    }
    decoded = decodeErc20Transfer(call.data);
  } catch (e) {
    if (e instanceof CallError && e.message.includes("approve")) {
      return { ok: false, code: "approve", reason: e.message };
    }
    return { ok: false, code: "malicious_selector", reason: e instanceof Error ? e.message : "malicious token call" };
  }
  if (decoded.amount > grant.maxAmount) {
    return { ok: false, code: "excessive_amount", reason: `amount ${decoded.amount} exceeds cap ${grant.maxAmount}` };
  }
  if (grant.recipient !== undefined && getAddress(decoded.to) !== getAddress(grant.recipient)) {
    return { ok: false, code: "recipient_mismatch", reason: "transfer recipient is not the granted recipient" };
  }
  if (call.quoteDepositAddress !== undefined) {
    if (getAddress(decoded.to) !== getAddress(call.quoteDepositAddress)) {
      return {
        ok: false,
        code: "quote_address_mismatch",
        reason: "transfer recipient is not the verified quote deposit address",
      };
    }
  }
  const gasCap = grant.maxGas > 0n ? grant.maxGas : SESSION_MAX_GAS;
  if (call.gasLimit > gasCap) {
    return { ok: false, code: "excessive_gas", reason: `gasLimit ${call.gasLimit} exceeds cap ${gasCap}` };
  }
  return { ok: true };
}
