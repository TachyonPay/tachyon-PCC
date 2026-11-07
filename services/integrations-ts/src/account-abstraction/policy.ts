/**
 * Builders for the restricted session permission (M3). Pure functions: they
 * produce the policy objects; nothing here signs or broadcasts.
 *
 * Design constraint discovered in M0: ZeroDev's CallPolicy constrains
 * `target` + function + args *per call*, evaluated against values fixed at
 * grant time. 1Click deposit addresses are allocated per quote, so a
 * permission granted at onboarding cannot name the deposit address. Options
 * for M3/M4 (to be proven on Sepolia before any mainnet grant):
 *   A. Per-payment permission grant (owner signs once per payment) — safe
 *      but defeats "no new owner signature".
 *   B. Payment-executor contract as the only allowed target, which itself
 *      enforces (token, amount cap, deposit address bound to a Rust-signed
 *      payment hash). Requires its own review (brief: contracts/ only if
 *      audited modules are insufficient).
 *   C. Session key constrained to `transfer(address,uint256)` on a token
 *      allowlist with amount <= cap but ANY recipient. **POC default:** a
 *      compromised server can send up to `maxAmount` per UserOp to any
 *      address (and can repeat; the cap is not cumulative). A later
 *      signature-bound executor (option B) should replace this.
 */
import { ParamCondition, toCallPolicy, toTimestampPolicy, CallPolicyVersion, type CallPolicyParams } from "@zerodev/permissions/policies";
import { erc20Abi, isAddress, type Address } from "viem";

export type CallPermission = NonNullable<CallPolicyParams["permissions"]>[number];

export interface Erc20TransferPermissionInput {
  token: Address;
  /**
   * Exact recipient. Omit for POC option C (any recipient). Do not omit in
   * production: the session signer could then drain `maxAmount` per UserOp.
   */
  recipient?: Address;
  /** Inclusive cap in token base units. Per call, not a remaining allowance. */
  maxAmount: bigint;
}

export class PolicyInputError extends Error {}

export function buildErc20TransferPermission(input: Erc20TransferPermissionInput): CallPermission {
  if (!isAddress(input.token)) throw new PolicyInputError("token must be an address");
  if (input.maxAmount <= 0n) throw new PolicyInputError("maxAmount must be > 0");
  const amountRule = { condition: ParamCondition.LESS_THAN_OR_EQUAL as const, value: input.maxAmount };
  const recipient = input.recipient;
  if (recipient !== undefined) {
    if (!isAddress(recipient)) throw new PolicyInputError("recipient must be an address");
    if (recipient.toLowerCase() === "0x0000000000000000000000000000000000000000") {
      throw new PolicyInputError("recipient must not be the zero address");
    }
  }
  return {
    target: input.token,
    valueLimit: 0n,
    abi: erc20Abi,
    functionName: "transfer" as const,
    args:
      recipient === undefined
        ? [null, amountRule]
        : [{ condition: ParamCondition.EQUAL as const, value: recipient }, amountRule],
  };
}

export function buildCallPolicy(permissions: CallPermission[]) {
  if (permissions.length === 0) throw new PolicyInputError("at least one permission required");
  return toCallPolicy({
    policyVersion: CallPolicyVersion.V0_0_4,
    permissions,
  });
}

export function buildExpiryPolicy(validUntilUnix: number, nowUnix = Math.floor(Date.now() / 1000)) {
  if (!Number.isInteger(validUntilUnix) || validUntilUnix <= nowUnix) {
    throw new PolicyInputError("validUntil must be an integer in the future");
  }
  if (validUntilUnix - nowUnix > 90 * 24 * 3600) {
    throw new PolicyInputError("validUntil may not exceed 90 days");
  }
  return toTimestampPolicy({ validAfter: nowUnix, validUntil: validUntilUnix });
}
