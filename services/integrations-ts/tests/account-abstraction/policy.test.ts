/**
 * Compile/shape smoke tests for the pinned ZeroDev stack. No network, no
 * signing. Proves the installed SDK exports what the M3 plan relies on.
 */
import { describe, expect, it } from "vitest";
import { zeroAddress } from "viem";
import { KERNEL_V3_3 } from "@zerodev/sdk/constants";
import { ENTRY_POINT, KERNEL_VERSION } from "../../src/account-abstraction/pins.js";
import { buildCallPolicy, buildErc20TransferPermission, buildExpiryPolicy, PolicyInputError } from "../../src/account-abstraction/policy.js";
import { onboardDecision, parseDelegation } from "../../src/account-abstraction/delegation.js";

const USDC_BASE = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" as const;
const DEPOSIT = "0x76b4c56085ED136a8744D52bE956396624a730E8" as const;

describe("pinned ZeroDev stack", () => {
  it("uses Kernel v3.3 on EntryPoint 0.7", () => {
    expect(KERNEL_VERSION).toBe(KERNEL_V3_3);
    expect(ENTRY_POINT.version).toBe("0.7");
    expect(ENTRY_POINT.address).toMatch(/^0x[0-9a-fA-F]{40}$/);
  });
});

describe("call policy builder", () => {
  it("produces a CallPolicy restricting transfer(to == deposit, amount <= cap)", () => {
    const perm = buildErc20TransferPermission({ token: USDC_BASE, recipient: DEPOSIT, maxAmount: 1_020_000n });
    expect(perm.target).toBe(USDC_BASE);
    expect(perm.valueLimit).toBe(0n);
    expect(perm.functionName).toBe("transfer");
    const policy = buildCallPolicy([perm]);
    expect(policy.getPolicyData()).toMatch(/^0x[0-9a-f]+$/i);
    expect(policy.policyParams.type).toBe("call");
    // The encoded data must embed the deposit address (lowercased hex).
    expect(policy.getPolicyData().toLowerCase()).toContain(DEPOSIT.slice(2).toLowerCase());
  });

  it("refuses wildcard / zero recipients, zero caps, bad addresses", () => {
    expect(() => buildErc20TransferPermission({ token: USDC_BASE, recipient: zeroAddress, maxAmount: 1n })).toThrow(PolicyInputError);
    expect(() => buildErc20TransferPermission({ token: USDC_BASE, recipient: DEPOSIT, maxAmount: 0n })).toThrow(PolicyInputError);
    expect(() => buildErc20TransferPermission({ token: "0x12" as never, recipient: DEPOSIT, maxAmount: 1n })).toThrow(PolicyInputError);
    expect(() => buildCallPolicy([])).toThrow(PolicyInputError);
  });

  it("expiry policy bounds validity to <= 90 days and rejects the past", () => {
    const now = 1_800_000_000;
    const p = buildExpiryPolicy(now + 3600, now);
    expect(p.policyParams.type).toBe("timestamp");
    expect(() => buildExpiryPolicy(now - 1, now)).toThrow(PolicyInputError);
    expect(() => buildExpiryPolicy(now + 91 * 24 * 3600, now)).toThrow(PolicyInputError);
  });
});

describe("7702 delegation inspection", () => {
  it("parses designators and refuses contract code", () => {
    expect(parseDelegation("0x")).toEqual({ state: "none" });
    expect(parseDelegation(undefined)).toEqual({ state: "none" });
    const target = "0x1111111111111111111111111111111111111111";
    expect(parseDelegation(`0xef0100${target.slice(2)}`)).toEqual({ state: "delegated", target });
    expect(parseDelegation("0x60806040")).toEqual({ state: "non_delegation_code", codeLen: 4 });
    expect(parseDelegation(`0xef0101${target.slice(2)}`).state).toBe("non_delegation_code");
  });

  it("onboarding decisions", () => {
    const kernel = "0x1111111111111111111111111111111111111111";
    const other = "0x2222222222222222222222222222222222222222";
    expect(onboardDecision({ state: "none" }, kernel)).toEqual({ decision: "install" });
    expect(onboardDecision({ state: "delegated", target: kernel }, kernel)).toEqual({ decision: "already_compatible" });
    expect(onboardDecision({ state: "delegated", target: other }, kernel, [other])).toEqual({ decision: "already_compatible" });
    expect(onboardDecision({ state: "delegated", target: other }, kernel)).toEqual({ decision: "requires_owner_review", current: other });
    expect(onboardDecision({ state: "non_delegation_code", codeLen: 9 }, kernel)).toEqual({ decision: "refuse" });
  });
});
