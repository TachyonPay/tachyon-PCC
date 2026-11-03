import { describe, expect, it } from "vitest";
import { onboardDecision, parseDelegation, readDelegation } from "../src/index.js";

const IMPL = "0x1111111111111111111111111111111111111111" as const;
const OTHER = "0x2222222222222222222222222222222222222222" as const;

describe("parseDelegation (shared vectors with Rust pcc-evm)", () => {
  it("empty / 0x -> none", () => {
    expect(parseDelegation(undefined)).toEqual({ kind: "none" });
    expect(parseDelegation("0x")).toEqual({ kind: "none" });
  });
  it("ef0100 || addr -> delegated (checksummed)", () => {
    expect(parseDelegation(`0xef0100${IMPL.slice(2)}`)).toEqual({ kind: "delegated", to: IMPL });
    expect(parseDelegation(`0xEF0100${IMPL.slice(2).toUpperCase()}`)).toEqual({ kind: "delegated", to: IMPL });
  });
  it("wrong length / wrong prefix -> contract", () => {
    expect(parseDelegation(`0xef0100${IMPL.slice(2)}00`)).toEqual({ kind: "contract" });
    expect(parseDelegation(`0xef0101${IMPL.slice(2)}`)).toEqual({ kind: "contract" });
    expect(parseDelegation("0x6080604052")).toEqual({ kind: "contract" });
  });
});

describe("onboardDecision", () => {
  it("fresh EOA -> offer", () => expect(onboardDecision({ kind: "none" }, [IMPL])).toEqual({ action: "offer_delegation" }));
  it("delegated to ours -> already_ours", () =>
    expect(onboardDecision({ kind: "delegated", to: IMPL }, [IMPL])).toEqual({ action: "already_ours" }));
  it("delegated elsewhere -> refuse (never overwrite)", () =>
    expect(onboardDecision({ kind: "delegated", to: OTHER }, [IMPL]).action).toBe("refuse"));
  it("contract account -> refuse", () => expect(onboardDecision({ kind: "contract" }, [IMPL]).action).toBe("refuse"));
});

describe("readDelegation", () => {
  it("uses only getCode", async () => {
    const client = { getCode: async () => `0xef0100${OTHER.slice(2)}` as const };
    expect(await readDelegation(client, IMPL)).toEqual({ kind: "delegated", to: OTHER });
  });
});
