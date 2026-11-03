/**
 * EIP-7702 delegation detection. Mirrors `backend/crates/evm/src/delegation.rs`
 * and `services/integrations-ts/src/account-abstraction/delegation.ts`; all
 * three must agree (tested here against the same vectors).
 *
 * An EOA that has delegated has code `0xef0100 || <20-byte address>` (23 bytes).
 */
import { type Address, type Hex, getAddress, isHex, size, slice } from "viem";

export const DELEGATION_PREFIX = "0xef0100" as const;

export type DelegationState =
  | { kind: "none" }
  | { kind: "delegated"; to: Address }
  | { kind: "contract" }; // non-7702 code: a real smart contract, not an EOA

export function parseDelegation(code: Hex | undefined | null): DelegationState {
  if (!code || code === "0x") return { kind: "none" };
  if (!isHex(code)) throw new Error("code must be hex");
  if (size(code) === 23 && code.toLowerCase().startsWith(DELEGATION_PREFIX)) {
    return { kind: "delegated", to: getAddress(slice(code, 3, 23)) };
  }
  return { kind: "contract" };
}

export type OnboardDecision =
  | { action: "offer_delegation" }
  | { action: "already_ours" }
  | { action: "refuse"; reason: string };

/**
 * Decide what the checkout UI may offer. We never overwrite a delegation we
 * do not recognise, and never touch real contract accounts.
 */
export function onboardDecision(state: DelegationState, knownImplementations: readonly Address[]): OnboardDecision {
  const known = new Set(knownImplementations.map((a) => a.toLowerCase()));
  switch (state.kind) {
    case "none":
      return { action: "offer_delegation" };
    case "delegated":
      return known.has(state.to.toLowerCase())
        ? { action: "already_ours" }
        : { action: "refuse", reason: `EOA already delegates to unfamiliar implementation ${state.to}` };
    case "contract":
      return { action: "refuse", reason: "address is a contract account, not an EOA" };
  }
}
