/**
 * EIP-7702 delegation inspection (TS mirror of backend/crates/evm/src/delegation.rs).
 * Used by the onboarding flow before asking the owner to sign anything.
 */
import { getAddress, type Address, type Hex } from "viem";

export type DelegationState =
  | { state: "none" }
  | { state: "delegated"; target: Address }
  | { state: "non_delegation_code"; codeLen: number };

const PREFIX = "0xef0100";

export function parseDelegation(code: Hex | undefined | null): DelegationState {
  if (!code || code === "0x") return { state: "none" };
  const body = code.toLowerCase();
  // 0x + 3 prefix bytes (6 hex) + 20 address bytes (40 hex) = 48 chars
  if (body.length === 48 && body.startsWith(PREFIX)) {
    return { state: "delegated", target: getAddress("0x" + body.slice(8)) };
  }
  return { state: "non_delegation_code", codeLen: (body.length - 2) / 2 };
}

export type OnboardDecision =
  | { decision: "install" }
  | { decision: "already_compatible" }
  | { decision: "requires_owner_review"; current: Address }
  | { decision: "refuse" };

export function onboardDecision(state: DelegationState, desired: Address, knownCompatible: Address[] = []): OnboardDecision {
  switch (state.state) {
    case "none":
      return { decision: "install" };
    case "delegated": {
      const t = state.target.toLowerCase();
      if (t === desired.toLowerCase() || knownCompatible.some((k) => k.toLowerCase() === t)) {
        return { decision: "already_compatible" };
      }
      return { decision: "requires_owner_review", current: state.target };
    }
    case "non_delegation_code":
      return { decision: "refuse" };
  }
}
