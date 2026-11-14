import { describe, expect, it } from "vitest";
import { decodeFunctionData, erc20Abi } from "viem";
import { encodeErc20Transfer, TRANSFER_SELECTOR } from "../src/index.js";

const TO = "0x049FdEf2550772B9F36F86757C27f5F982D0970D" as const;

describe("encodeErc20Transfer", () => {
  it("encodes transfer(address,uint256)", () => {
    const data = encodeErc20Transfer(TO, 1_000_000n);
    expect(data.startsWith(TRANSFER_SELECTOR)).toBe(true);
    const decoded = decodeFunctionData({ abi: erc20Abi, data });
    expect(decoded.functionName).toBe("transfer");
    expect(decoded.args).toEqual([TO, 1_000_000n]);
  });

  it("refuses a zero amount", () => {
    expect(() => encodeErc20Transfer(TO, 0n)).toThrow(/greater than 0/);
  });
});
