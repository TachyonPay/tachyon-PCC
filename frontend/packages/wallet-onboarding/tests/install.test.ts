import { describe, expect, it } from "vitest";
import { KERNEL_7702_IMPLEMENTATION, installKernelDelegation, installPlan } from "../src/index.js";
import type { Eip1193Provider } from "../src/index.js";

const OTHER = "0x2222222222222222222222222222222222222222";
const OWNER = "0x049FdEf2550772B9F36F86757C27f5F982D0970D" as const;
const KERNEL_CODE = `0xef0100${KERNEL_7702_IMPLEMENTATION.slice(2).toLowerCase()}` as const;
const OTHER_CODE = `0xef0100${OTHER.slice(2)}` as const;

function provider(codes: string[], extra: Record<string, unknown> = {}): { calls: string[]; rpc: Eip1193Provider } {
  const calls: string[] = [];
  const rpc: Eip1193Provider = {
    request: async ({ method }) => {
      calls.push(method);
      if (method === "wallet_switchEthereumChain") return null;
      if (method === "eth_getCode") return codes.shift() ?? "0x";
      if (method === "eth_getTransactionCount") return "0x3";
      if (method === "eth_signAuthorization" || method === "wallet_signAuthorization") {
        return {
          address: KERNEL_7702_IMPLEMENTATION,
          r: `0x${"11".repeat(32)}`,
          s: `0x${"22".repeat(32)}`,
          yParity: "0x1",
          ...extra,
        };
      }
      if (method === "eth_sendTransaction") return "0xabc";
      throw new Error(`unsupported ${method}`);
    },
  };
  return { calls, rpc };
}

describe("installPlan", () => {
  it("offers a fresh EOA and skips Kernel", () => {
    expect(installPlan("0x", KERNEL_7702_IMPLEMENTATION)).toEqual({ action: "install" });
    expect(installPlan(KERNEL_CODE, KERNEL_7702_IMPLEMENTATION)).toEqual({ action: "skip" });
  });
  it("refuses any other code", () => {
    expect(installPlan(OTHER_CODE, KERNEL_7702_IMPLEMENTATION).action).toBe("refuse");
    expect(installPlan("0x6080604052", KERNEL_7702_IMPLEMENTATION).action).toBe("refuse");
  });
});

describe("installKernelDelegation", () => {
  it("sends one type-4 transaction when the account is empty on every read", async () => {
    const { calls, rpc } = provider(["0x", "0x", "0x"]);
    const tx = await installKernelDelegation(rpc, OWNER, 8453);
    expect(tx).toBe("0xabc");
    expect(calls.filter((m) => m === "eth_sendTransaction")).toEqual(["eth_sendTransaction"]);
  });

  it("does nothing when Kernel is already installed", async () => {
    const { calls, rpc } = provider([KERNEL_CODE]);
    expect(await installKernelDelegation(rpc, OWNER, 8453)).toBeNull();
    expect(calls).not.toContain("eth_signAuthorization");
    expect(calls).not.toContain("eth_sendTransaction");
  });

  it("does not sign when the account already delegates elsewhere", async () => {
    const { calls, rpc } = provider([OTHER_CODE]);
    await expect(installKernelDelegation(rpc, OWNER, 8453)).rejects.toThrow(/unfamiliar/);
    expect(calls).not.toContain("eth_signAuthorization");
  });

  it("stops before sending if code appears after the signature", async () => {
    const { calls, rpc } = provider(["0x", "0x", OTHER_CODE]);
    await expect(installKernelDelegation(rpc, OWNER, 8453)).rejects.toThrow(/code changed/);
    expect(calls).not.toContain("eth_sendTransaction");
  });

  it("rejects an authorization for a different contract", async () => {
    const { calls, rpc } = provider(["0x", "0x", "0x"], { address: OTHER });
    await expect(installKernelDelegation(rpc, OWNER, 8453)).rejects.toThrow(/different contract/);
    expect(calls).not.toContain("eth_sendTransaction");
  });
});
