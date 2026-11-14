import { describe, expect, it } from "vitest";
import {
  KERNEL_7702_IMPLEMENTATION,
  KERNEL_INSTALL_UNSUPPORTED,
  installKernelDelegation,
  installPlan,
  parseRpcQuantity,
} from "../src/index.js";
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

describe("parseRpcQuantity", () => {
  it("accepts hex, decimal, number, and bigint", () => {
    expect(parseRpcQuantity("0x3")).toBe(3n);
    expect(parseRpcQuantity("3")).toBe(3n);
    expect(parseRpcQuantity(3)).toBe(3n);
    expect(parseRpcQuantity(0n)).toBe(0n);
    expect(parseRpcQuantity(undefined)).toBeNull();
    expect(parseRpcQuantity(null)).toBeNull();
    expect(parseRpcQuantity({})).toBeNull();
  });
});

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
    const { calls, rpc } = provider(["0x", "0x", "0x", KERNEL_CODE]);
    const tx = await installKernelDelegation(rpc, OWNER, 8453, KERNEL_7702_IMPLEMENTATION, { confirmMs: 0 });
    expect(tx).toBe("0xabc");
    expect(calls.filter((m) => m === "eth_sendTransaction")).toEqual(["eth_sendTransaction"]);
  });

  it("accepts a numeric nonce from WalletConnect", async () => {
    const { rpc } = provider(["0x", "0x", "0x", KERNEL_CODE]);
    const numeric: Eip1193Provider = {
      request: async (args) => {
        if (args.method === "eth_getTransactionCount") return 3;
        return rpc.request(args);
      },
    };
    expect(await installKernelDelegation(numeric, OWNER, 8453, KERNEL_7702_IMPLEMENTATION, { confirmMs: 0 })).toBe("0xabc");
  });

  it("sends an unsigned type-4 when the wallet has no signAuthorization method", async () => {
    const sent: unknown[] = [];
    const { rpc } = provider(["0x", "0x", "0x", KERNEL_CODE]);
    const wc: Eip1193Provider = {
      request: async (args) => {
        if (args.method === "eth_signAuthorization" || args.method === "wallet_signAuthorization") {
          throw new Error(`The method "${args.method}" does not exist / is not available`);
        }
        if (args.method === "eth_sendTransaction") {
          sent.push(args.params?.[0]);
        }
        return rpc.request(args);
      },
    };
    expect(await installKernelDelegation(wc, OWNER, 8453, KERNEL_7702_IMPLEMENTATION, { confirmMs: 0 })).toBe("0xabc");
    const tx = sent[0] as { authorizationList: Array<{ r?: string }> };
    expect(tx.authorizationList[0]?.r).toBeUndefined();
  });

  it("explains when MetaMask refuses EIP-7702 from a website", async () => {
    const { rpc } = provider(["0x", "0x", "0x"]);
    const mm: Eip1193Provider = {
      request: async (args) => {
        if (args.method === "eth_signAuthorization" || args.method === "wallet_signAuthorization") {
          throw new Error(`The method "${args.method}" does not exist / is not available`);
        }
        if (args.method === "eth_sendTransaction") {
          throw new Error("External EIP-7702 transactions are not supported.");
        }
        return rpc.request(args);
      },
    };
    await expect(installKernelDelegation(mm, OWNER, 8453, KERNEL_7702_IMPLEMENTATION, { confirmMs: 0 })).rejects.toThrow(
      KERNEL_INSTALL_UNSUPPORTED,
    );
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

  it("uses a pre-signed Privy authorization and does not call wallet sign methods", async () => {
    const sent: unknown[] = [];
    const { calls, rpc } = provider(["0x", "0x", "0x", KERNEL_CODE]);
    const wrapped: Eip1193Provider = {
      request: async (args) => {
        if (args.method === "eth_sendTransaction") sent.push(args.params?.[0]);
        return rpc.request(args);
      },
    };
    const tx = await installKernelDelegation(wrapped, OWNER, 8453, KERNEL_7702_IMPLEMENTATION, {
      confirmMs: 0,
      authorization: {
        address: KERNEL_7702_IMPLEMENTATION,
        r: `0x${"aa".repeat(32)}`,
        s: `0x${"bb".repeat(32)}`,
        yParity: 1,
      },
    });
    expect(tx).toBe("0xabc");
    expect(calls).not.toContain("eth_sign7702Authorization");
    expect(calls).not.toContain("eth_signAuthorization");
    expect(calls).not.toContain("wallet_signAuthorization");
    const body = sent[0] as {
      type: number;
      authorization_list: Array<{ contract: string; y_parity: number; nonce: number }>;
    };
    expect(body.type).toBe(4);
    expect(body.authorization_list[0]?.contract).toBe(KERNEL_7702_IMPLEMENTATION);
    expect(body.authorization_list[0]?.y_parity).toBe(1);
    expect(body.authorization_list[0]?.nonce).toBe(3);
  });

  it("sends the type-4 via a Privy send callback instead of eth_sendTransaction", async () => {
    const { calls, rpc } = provider(["0x", "0x", "0x", KERNEL_CODE]);
    let sentNonce: bigint | undefined;
    const tx = await installKernelDelegation(rpc, OWNER, 8453, KERNEL_7702_IMPLEMENTATION, {
      confirmMs: 0,
      authorization: {
        address: KERNEL_7702_IMPLEMENTATION,
        r: `0x${"aa".repeat(32)}`,
        s: `0x${"bb".repeat(32)}`,
        yParity: 1,
      },
      send: async (req) => {
        sentNonce = req.nonce;
        return "0xdef" as const;
      },
    });
    expect(tx).toBe("0xdef");
    expect(sentNonce).toBe(3n);
    expect(calls).not.toContain("eth_sendTransaction");
  });

  it("passes the account nonce into an authorization callback", async () => {
    const { rpc } = provider(["0x", "0x", "0x", KERNEL_CODE]);
    let seen: bigint | undefined;
    await installKernelDelegation(rpc, OWNER, 8453, KERNEL_7702_IMPLEMENTATION, {
      confirmMs: 0,
      authorization: async ({ nonce }) => {
        seen = nonce;
        return {
          address: KERNEL_7702_IMPLEMENTATION,
          r: `0x${"aa".repeat(32)}`,
          s: `0x${"bb".repeat(32)}`,
          yParity: 0,
        };
      },
    });
    expect(seen).toBe(3n);
  });
});
