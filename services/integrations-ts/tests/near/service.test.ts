import { beforeEach, describe, expect, it } from "vitest";
import { WorkerError, err } from "../../src/errors.js";
import { NearService, type QuoteRequestDto } from "../../src/near/service.js";
import { FakeOneClickClient, SOL_NATIVE, USDC_ARB, USDC_BASE } from "../helpers/fakeClient.js";
import { fakeQuoteResponse, makeTestSigner, type TestSigner } from "../helpers/signer.js";

const NOW = new Date("2026-11-03T12:00:00.000Z");
const RECIPIENT = "0x1111111111111111111111111111111111111111";
const REFUND = "0x2222222222222222222222222222222222222222";

function dto(over: Partial<QuoteRequestDto> = {}): QuoteRequestDto {
  return {
    dry: false,
    authorizationRef: "auth-ref-0001",
    originAssetId: USDC_BASE,
    destinationAssetId: USDC_ARB,
    amountOut: "1000000",
    recipient: RECIPIENT,
    refundTo: REFUND,
    confidentiality: "basic",
    slippageBps: 100,
    deadline: new Date(NOW.getTime() + 2 * 3600_000).toISOString(),
    ...over,
  };
}

async function expectCode(p: Promise<unknown>, code: string, msg?: RegExp) {
  try {
    await p;
    throw new Error("expected rejection");
  } catch (e) {
    expect(e, String(e)).toBeInstanceOf(WorkerError);
    expect((e as WorkerError).code).toBe(code);
    if (msg) expect((e as WorkerError).message).toMatch(msg);
  }
}

let signer: TestSigner;
let client: FakeOneClickClient;
let svc: NearService;

beforeEach(() => {
  signer = makeTestSigner();
  client = new FakeOneClickClient(signer);
  svc = new NearService(client, { clock: { now: () => NOW }, managerPublicKey: signer.publicKey });
});

describe("NearService.quote — request construction", () => {
  it("builds EXACT_OUTPUT / ORIGIN_CHAIN / DESTINATION_CHAIN / ORIGIN_CHAIN refund / confidentiality basic", async () => {
    const q = await svc.quote(dto());
    const sent = client.lastRequest!;
    expect(sent).toMatchObject({
      dry: false,
      swapType: "EXACT_OUTPUT",
      depositType: "ORIGIN_CHAIN",
      recipientType: "DESTINATION_CHAIN",
      refundType: "ORIGIN_CHAIN",
      depositMode: "SIMPLE",
      confidentiality: "basic",
      originAsset: USDC_BASE,
      destinationAsset: USDC_ARB,
      amount: "1000000",
      recipient: RECIPIENT,
      refundTo: REFUND,
      slippageTolerance: 100,
    });
    expect(q.signatureVerified).toBe(true);
    expect(q.swapType).toBe("EXACT_OUTPUT");
    expect(q.confidentiality).toBe("basic");
    expect(q.amountOut).toBe("1000000");
    expect(BigInt(q.amountIn)).toBeGreaterThan(BigInt(q.minAmountIn));
    expect(q.depositAddress).toMatch(/^0x[0-9a-f]{40}$/);
    expect(q.deadline).toBeDefined();
    expect(JSON.parse(q.rawResponseJson).signature).toBe(q.signature);
  });

  it("dry quotes carry no deposit address", async () => {
    const q = await svc.quote(dto({ dry: true }));
    expect(q.dry).toBe(true);
    expect(q.depositAddress).toBeUndefined();
    expect(q.deadline).toBeUndefined();
  });
});

describe("NearService.quote — input validation (INVALID_REQUEST)", () => {
  it("rejects unsupported / unknown tokens", async () => {
    await expectCode(svc.quote(dto({ originAssetId: "nep141:unknown.near" })), "INVALID_REQUEST", /originAssetId/);
    await expectCode(svc.quote(dto({ destinationAssetId: "nep141:unknown.near" })), "INVALID_REQUEST", /destinationAssetId/);
  });

  it("accepts non-EVM asset ids only if 1Click lists them (chain filtering is Rust's job)", async () => {
    // SOL is in the token list; TS does not reject it — Rust filters by chain mapping.
    await expect(svc.quote(dto({ originAssetId: SOL_NATIVE }))).resolves.toBeDefined();
  });

  it("rejects same origin/destination (reversed-chain misuse)", async () => {
    await expectCode(svc.quote(dto({ destinationAssetId: USDC_BASE })), "INVALID_REQUEST", /differ/);
  });

  it("rejects zero / non-integer amounts", async () => {
    await expectCode(svc.quote(dto({ amountOut: "0" })), "INVALID_REQUEST", /amountOut/);
    await expectCode(svc.quote(dto({ amountOut: "1.5" })), "INVALID_REQUEST");
    await expectCode(svc.quote({ ...dto(), amountOut: 1000000 } as unknown), "INVALID_REQUEST");
  });

  it("rejects missing / invalid recipient and refund addresses", async () => {
    await expectCode(svc.quote({ ...dto(), recipient: "" } as unknown), "INVALID_REQUEST");
    await expectCode(svc.quote(dto({ recipient: "0x123" })), "INVALID_REQUEST");
    await expectCode(svc.quote({ ...dto(), refundTo: "bc1qnotevm" } as unknown), "INVALID_REQUEST");
  });

  it("rejects deadlines that are too soon, bad confidentiality, excessive slippage, unknown fields", async () => {
    await expectCode(svc.quote(dto({ deadline: new Date(NOW.getTime() + 60_000).toISOString() })), "INVALID_REQUEST", /deadline/);
    await expectCode(svc.quote({ ...dto(), confidentiality: "secret" } as unknown), "INVALID_REQUEST");
    await expectCode(svc.quote(dto({ slippageBps: 5000 })), "INVALID_REQUEST");
    await expectCode(svc.quote({ ...dto(), depositAddress: "0xattacker" } as unknown), "INVALID_REQUEST");
  });

  it("never lets the caller pass a raw 1Click request", async () => {
    await expectCode(svc.quote({ ...dto(), swapType: "EXACT_INPUT" } as unknown), "INVALID_REQUEST");
    await expectCode(svc.quote({ ...dto(), recipientType: "INTENTS" } as unknown), "INVALID_REQUEST");
  });
});

describe("NearService.quote — upstream failures", () => {
  it("no API key -> UPSTREAM_UNAUTHORIZED without calling upstream", async () => {
    client.hasApiKey = false;
    await expectCode(svc.quote(dto()), "UPSTREAM_UNAUTHORIZED");
    expect(client.calls.quote).toBe(0);
  });

  it("propagates timeout / rate limit / unauthorized / no route / malformed", async () => {
    for (const e of [err.upstreamTimeout(), err.upstreamRateLimited(), err.upstreamUnauthorized(), err.noRoute("no quotes"), err.malformedUpstream("bad")]) {
      client.nextError = e;
      await expectCode(svc.quote(dto()), e.code);
    }
  });

  it("releases the authorizationRef when upstream fails, so a retry can succeed", async () => {
    client.nextError = err.upstreamTimeout();
    await expectCode(svc.quote(dto()), "UPSTREAM_TIMEOUT");
    await expect(svc.quote(dto())).resolves.toBeDefined();
  });
});

describe("NearService.quote — signature & echo binding", () => {
  it("rejects a quote signed by a different key", async () => {
    const other = makeTestSigner();
    client.quoteHook = (req) => fakeQuoteResponse(other, req);
    await expectCode(svc.quote(dto()), "SIGNATURE_INVALID");
  });

  it("rejects a modified deposit address (MITM)", async () => {
    client.quoteHook = (_req, signed) => ({ ...signed, quote: { ...signed.quote, depositAddress: "0x9999999999999999999999999999999999999999" } });
    await expectCode(svc.quote(dto()), "SIGNATURE_INVALID");
  });

  it("rejects a non-confidential result when basic was requested (echo downgrade)", async () => {
    client.quoteHook = (_req, signed) => ({ ...signed, quoteRequest: { ...signed.quoteRequest, confidentiality: "public" } });
    await expectCode(svc.quote(dto()), "SIGNATURE_INVALID", /confidentiality/);
  });

  it("rejects quote/order mismatch in the echoed recipient (signature covers it)", async () => {
    client.quoteHook = (_req, signed) => ({ ...signed, quoteRequest: { ...signed.quoteRequest, recipient: "0x3333333333333333333333333333333333333333" } });
    await expectCode(svc.quote(dto()), "SIGNATURE_INVALID");
  });

  it("rejects a replayed response for a different request", async () => {
    const first = await svc.quote(dto({ dry: true, amountOut: "1000000" }));
    const replay = JSON.parse(first.rawResponseJson);
    client.quoteHook = () => replay;
    await expectCode(svc.quote(dto({ dry: true, amountOut: "2000000", authorizationRef: "auth-ref-0002" })), "SIGNATURE_INVALID");
  });
});

describe("NearService — authorizationRef single use", () => {
  it("rejects a second executable quote with the same ref", async () => {
    await svc.quote(dto());
    await expectCode(svc.quote(dto()), "AUTH_REF_REUSED");
  });

  it("dry quotes do not consume refs", async () => {
    await svc.quote(dto({ dry: true }));
    await svc.quote(dto({ dry: true }));
    await expect(svc.quote(dto())).resolves.toBeDefined();
  });

  it("concurrent duplicates: exactly one succeeds", async () => {
    const results = await Promise.allSettled([svc.quote(dto()), svc.quote(dto())]);
    const ok = results.filter((r) => r.status === "fulfilled").length;
    expect(ok).toBe(1);
    expect(client.calls.quote).toBe(1);
  });
});

describe("NearService.verifyRaw / status / depositSubmitted / assets", () => {
  it("re-verifies stored raw JSON and rejects tampered or garbage input", async () => {
    const q = await svc.quote(dto());
    const again = svc.verifyRaw(q.rawResponseJson);
    expect(again.depositAddress).toBe(q.depositAddress);
    const tampered = JSON.parse(q.rawResponseJson);
    tampered.quote.amountOut = "999";
    await expectCode(Promise.resolve().then(() => svc.verifyRaw(JSON.stringify(tampered))), "SIGNATURE_INVALID");
    await expectCode(Promise.resolve().then(() => svc.verifyRaw("{not json")), "INVALID_REQUEST");
    await expectCode(Promise.resolve().then(() => svc.verifyRaw("{}")), "INVALID_REQUEST");
  });

  it("status re-verifies the embedded quote and maps tx hashes", async () => {
    const q = await svc.quote(dto());
    const embedded = JSON.parse(q.rawResponseJson);
    client.statuses.set(q.depositAddress!, {
      correlationId: "c",
      quoteResponse: embedded,
      status: "SUCCESS",
      updatedAt: "2026-11-03T12:30:00.000Z",
      swapDetails: { originChainTxHashes: [{ hash: "0xaaa" }], destinationChainTxHashes: [{ hash: "0xbbb" }], amountOut: "1000000" },
    });
    const s = await svc.status(q.depositAddress!);
    expect(s).toMatchObject({ status: "SUCCESS", originTxHashes: ["0xaaa"], destinationTxHashes: ["0xbbb"], amountOut: "1000000" });

    // Live 1Click omits quoteWaitingTimeMs from the status echo.
    delete embedded.quoteRequest.quoteWaitingTimeMs;
    const dropped = await svc.status(q.depositAddress!);
    expect(dropped.status).toBe("SUCCESS");

    embedded.quote.depositAddress = "0x9999999999999999999999999999999999999999";
    await expectCode(svc.status(q.depositAddress!), "SIGNATURE_INVALID");
    await expectCode(svc.status(""), "INVALID_REQUEST");
  });

  it("depositSubmitted validates the tx hash shape", async () => {
    await expectCode(svc.depositSubmitted("0xdep", "0x123"), "INVALID_REQUEST");
    await svc.depositSubmitted("0xdep", "0x" + "ab".repeat(32));
    expect(client.submitted).toHaveLength(1);
  });

  it("assets passes through 1Click entries and caches", async () => {
    const a = await svc.assets();
    expect(a.tokens.map((t) => t.assetId)).toEqual([USDC_BASE, USDC_ARB, SOL_NATIVE]);
    expect(a.tokens[0]?.priceUsd).toBe("1");
    await svc.assets();
    expect(client.calls.tokens).toBe(1);
  });
});
