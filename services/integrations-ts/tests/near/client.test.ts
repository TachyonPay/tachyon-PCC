import { describe, expect, it } from "vitest";
import { WorkerError } from "../../src/errors.js";
import { OneClickClient, type FetchLike } from "../../src/near/client.js";
import fixtures from "../../fixtures/one-click-staging-quotes.json" with { type: "json" };

function mkFetch(handler: (url: string, init: RequestInit) => Response | Promise<Response>): { fetch: FetchLike; calls: { url: string; init: RequestInit }[] } {
  const calls: { url: string; init: RequestInit }[] = [];
  return {
    calls,
    fetch: async (url, init) => {
      calls.push({ url, init });
      return handler(url, init);
    },
  };
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

async function expectCode(p: Promise<unknown>, code: string) {
  try {
    await p;
    throw new Error("expected rejection");
  } catch (e) {
    expect(e).toBeInstanceOf(WorkerError);
    expect((e as WorkerError).code).toBe(code);
  }
}

const REQ = fixtures.dryQuote.quoteRequest as never;

describe("OneClickClient", () => {
  it("sends X-API-Key (not a JWT) and JSON body to /v0/quote", async () => {
    const f = mkFetch(() => json(200, fixtures.dryQuote));
    const c = new OneClickClient({ baseUrl: "https://1click.example", apiKey: "k-secret", timeoutMs: 1000, fetchImpl: f.fetch });
    const r = await c.postQuote(REQ);
    expect(f.calls[0]?.url).toBe("https://1click.example/v0/quote");
    const headers = f.calls[0]?.init.headers as Record<string, string>;
    expect(headers["X-API-Key"]).toBe("k-secret");
    expect(headers["Authorization"]).toBeUndefined();
    expect(f.calls[0]?.init.redirect).toBe("error");
    expect(r.parsed.correlationId).toBe(fixtures.dryQuote.correlationId);
    expect(r.rawJson).toBe(JSON.stringify(fixtures.dryQuote));
  });

  it("omits the header when no key is configured", async () => {
    const f = mkFetch(() => json(200, []));
    const c = new OneClickClient({ baseUrl: "https://x", apiKey: undefined, timeoutMs: 1000, fetchImpl: f.fetch });
    await c.getTokens();
    expect((f.calls[0]?.init.headers as Record<string, string>)["X-API-Key"]).toBeUndefined();
  });

  it("maps 401 -> UPSTREAM_UNAUTHORIZED, 429 -> UPSTREAM_RATE_LIMITED, 5xx -> UPSTREAM_UNAVAILABLE", async () => {
    for (const [status, code] of [
      [401, "UPSTREAM_UNAUTHORIZED"],
      [403, "UPSTREAM_UNAUTHORIZED"],
      [429, "UPSTREAM_RATE_LIMITED"],
      [500, "UPSTREAM_UNAVAILABLE"],
      [503, "UPSTREAM_UNAVAILABLE"],
    ] as const) {
      const c = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: mkFetch(() => new Response("", { status })).fetch });
      await expectCode(c.postQuote(REQ), code);
    }
  });

  it("maps 400 'no quotes' to NO_ROUTE and other 400s to UPSTREAM_REJECTED", async () => {
    const c1 = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: mkFetch(() => json(400, { message: "No quotes available for this pair" })).fetch });
    await expectCode(c1.postQuote(REQ), "NO_ROUTE");
    const c2 = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: mkFetch(() => json(400, { message: "amount must be an integer string" })).fetch });
    await expectCode(c2.postQuote(REQ), "UPSTREAM_REJECTED");
  });

  it("times out via AbortController -> UPSTREAM_TIMEOUT", async () => {
    const f: FetchLike = (_url, init) =>
      new Promise((_resolve, reject) => {
        init.signal?.addEventListener("abort", () => {
          const e = new Error("aborted");
          e.name = "AbortError";
          reject(e);
        });
      });
    const c = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 50, fetchImpl: f });
    await expectCode(c.postQuote(REQ), "UPSTREAM_TIMEOUT");
  });

  it("network failure -> UPSTREAM_UNAVAILABLE", async () => {
    const c = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: async () => { throw new TypeError("fetch failed"); } });
    await expectCode(c.getTokens(), "UPSTREAM_UNAVAILABLE");
  });

  it("non-JSON and schema-violating bodies -> MALFORMED_UPSTREAM", async () => {
    const c1 = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: mkFetch(() => new Response("<html>", { status: 200 })).fetch });
    await expectCode(c1.postQuote(REQ), "MALFORMED_UPSTREAM");

    const truncated = structuredClone(fixtures.dryQuote) as Record<string, unknown>;
    delete truncated["signature"];
    const c2 = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: mkFetch(() => json(200, truncated)).fetch });
    await expectCode(c2.postQuote(REQ), "MALFORMED_UPSTREAM");

    const numericAmount = structuredClone(fixtures.dryQuote) as { quote: Record<string, unknown> };
    numericAmount.quote["amountIn"] = 10000; // number instead of string
    const c3 = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: mkFetch(() => json(200, numericAmount)).fetch });
    await expectCode(c3.postQuote(REQ), "MALFORMED_UPSTREAM");

    const decimalAmount = structuredClone(fixtures.dryQuote) as { quote: Record<string, unknown> };
    decimalAmount.quote["amountOut"] = "5.93";
    const c4 = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: mkFetch(() => json(200, decimalAmount)).fetch });
    await expectCode(c4.postQuote(REQ), "MALFORMED_UPSTREAM");
  });

  it("status: passes depositMemo and rejects unknown status enums", async () => {
    const good = {
      correlationId: "c",
      quoteResponse: fixtures.nonDryQuote,
      status: "PROCESSING",
      updatedAt: "2026-11-03T12:00:00.000Z",
      swapDetails: { originChainTxHashes: [{ hash: "0xaaa" }], destinationChainTxHashes: [] },
    };
    const f = mkFetch(() => json(200, good));
    const c = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: f.fetch });
    const s = await c.getStatus("bc1qdeposit", "memo-1");
    expect(f.calls[0]?.url).toBe("https://x/v0/status?depositAddress=bc1qdeposit&depositMemo=memo-1");
    expect(s.status).toBe("PROCESSING");

    const bad = { ...good, status: "DONE" };
    const c2 = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: mkFetch(() => json(200, bad)).fetch });
    await expectCode(c2.getStatus("x"), "MALFORMED_UPSTREAM");

    const c3 = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: mkFetch(() => json(404, { message: "not found" })).fetch });
    await expectCode(c3.getStatus("x"), "UPSTREAM_REJECTED");
  });

  it("tokens: validates the array schema", async () => {
    const c = new OneClickClient({ baseUrl: "https://x", apiKey: "k", timeoutMs: 1000, fetchImpl: mkFetch(() => json(200, [{ assetId: "a" }])).fetch });
    await expectCode(c.getTokens(), "MALFORMED_UPSTREAM");
  });
});
