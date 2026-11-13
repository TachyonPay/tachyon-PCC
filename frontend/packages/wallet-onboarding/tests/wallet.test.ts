import { describe, expect, it } from "vitest";
import { hashTypedData, recoverTypedDataAddress, type TypedDataDefinition } from "viem";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";
import { chainIdHex, toSignTypedDataV4, walletSource } from "../src/wallet.js";

const enable = {
  domain: { name: "Kernel", version: "0.3.3", chainId: 8453, verifyingContract: "0x049FdEf2550772B9F36F86757C27f5F982D0970D" },
  types: {
    Enable: [
      { name: "validationId", type: "bytes21" },
      { name: "nonce", type: "uint32" },
      { name: "hook", type: "address" },
      { name: "validatorData", type: "bytes" },
      { name: "hookData", type: "bytes" },
      { name: "selectorData", type: "bytes" },
    ],
  },
  primaryType: "Enable",
  message: {
    validationId: "0x020000000000000000000000000000000000000001",
    nonce: "1",
    hook: "0x0000000000000000000000000000000000000001",
    validatorData: "0x1234",
    hookData: "0x",
    selectorData: "0xe9ae5c53",
  },
};

describe("walletSource", () => {
  it("prefers injected, then a well-formed WalletConnect project id", () => {
    expect(walletSource({ hasInjected: true, walletConnectProjectId: "a".repeat(32) })).toBe("injected");
    expect(walletSource({ hasInjected: false, walletConnectProjectId: "0123456789abcdef0123456789ABCDEF" })).toBe("walletconnect");
    expect(walletSource({ hasInjected: false, walletConnectProjectId: "" })).toBe("none");
    expect(walletSource({ hasInjected: false, walletConnectProjectId: "not-a-project-id" })).toBe("none");
    expect(walletSource({ hasInjected: false })).toBe("none");
  });
});

describe("chainIdHex", () => {
  it("formats wallet_switchEthereumChain ids", () => {
    expect(chainIdHex(8453)).toBe("0x2105");
    expect(chainIdHex(42161)).toBe("0xa4b1");
    expect(() => chainIdHex(0)).toThrow();
  });
});

describe("toSignTypedDataV4", () => {
  it("adds EIP712Domain for exactly the present fields, in canonical order", () => {
    const v4 = JSON.parse(toSignTypedDataV4(enable));
    expect(v4.types.EIP712Domain).toEqual([
      { name: "name", type: "string" },
      { name: "version", type: "string" },
      { name: "chainId", type: "uint256" },
      { name: "verifyingContract", type: "address" },
    ]);
    expect(v4.types.Enable).toEqual(enable.types.Enable);
    expect(v4.message).toEqual(enable.message);
  });

  it("hashes identically to the server payload, so a wallet signature recovers to the payer", async () => {
    const v4 = JSON.parse(toSignTypedDataV4(enable)) as TypedDataDefinition;
    expect(hashTypedData(v4)).toBe(hashTypedData(enable as unknown as TypedDataDefinition));
    const payer = privateKeyToAccount(generatePrivateKey());
    const signature = await payer.signTypedData(v4);
    expect(await recoverTypedDataAddress({ ...(enable as unknown as TypedDataDefinition), signature })).toBe(payer.address);
  });

  it("rejects malformed payloads", () => {
    expect(() => toSignTypedDataV4({})).toThrow();
    expect(() => toSignTypedDataV4(null)).toThrow();
  });
});
