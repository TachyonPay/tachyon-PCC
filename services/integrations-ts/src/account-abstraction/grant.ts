/**
 * Compose the session-key policies installed at onboarding.
 *
 * POC: option C (token + amount cap, any recipient) + timestamp expiry +
 * gas cap + rate-limit count=1 so the grant is single-use. Never sudo.
 */
import { toGasPolicy, toRateLimitPolicy } from "@zerodev/permissions/policies";
import type { Address } from "viem";
import { buildCallPolicy, buildErc20TransferPermission, buildExpiryPolicy, type CallPermission } from "./policy.js";
import { KERNEL_7702_DELEGATION_ADDRESS, SESSION_MAX_GAS, SESSION_MAX_GAS_WEI } from "./pins.js";
import type { SessionGrant } from "./validate.js";

export interface BuildGrantInput {
  owner: Address;
  sessionSigner: Address;
  chainId: number;
  token: Address;
  maxAmount: bigint;
  validUntil: number;
  nowUnix?: number;
  maxGas?: bigint;
  maxGasWei?: bigint;
  recipient?: Address;
  /** Session UserOps allowed in `interval`. Default 1 (single-use). */
  rateLimitCount?: number;
}

export function buildGrant(input: BuildGrantInput): SessionGrant {
  const now = input.nowUnix ?? Math.floor(Date.now() / 1000);
  buildExpiryPolicy(input.validUntil, now); // validates bounds
  const grant: SessionGrant = {
    owner: input.owner,
    sessionSigner: input.sessionSigner,
    chainId: input.chainId,
    token: input.token,
    maxAmount: input.maxAmount,
    validUntil: input.validUntil,
    revoked: false,
    kernelImplementation: KERNEL_7702_DELEGATION_ADDRESS,
    maxGas: input.maxGas ?? SESSION_MAX_GAS,
    maxGasWei: input.maxGasWei ?? SESSION_MAX_GAS_WEI,
    rateLimitCount: input.rateLimitCount ?? 1,
  };
  if (input.recipient !== undefined) grant.recipient = input.recipient;
  return grant;
}

export function buildSessionPolicies(grant: SessionGrant, nowUnix = Math.floor(Date.now() / 1000)) {
  const perm: CallPermission = buildErc20TransferPermission({
    token: grant.token,
    maxAmount: grant.maxAmount,
    ...(grant.recipient !== undefined ? { recipient: grant.recipient } : {}),
  });
  const call = buildCallPolicy([perm]);
  const expiry = buildExpiryPolicy(grant.validUntil, nowUnix);
  const gas = toGasPolicy({ allowed: grant.maxGasWei });
  const window = grant.validUntil - nowUnix;
  const count = grant.rateLimitCount && grant.rateLimitCount > 0 ? grant.rateLimitCount : 1;
  const rate = toRateLimitPolicy({ count, interval: window > 0 ? window : 1 });
  const policies = [call, expiry, gas, rate];
  for (const p of policies) {
    if (p.policyParams.type === "sudo") {
      throw new Error("refusing to install a sudo session policy");
    }
  }
  return policies;
}
