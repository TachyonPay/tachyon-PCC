/**
 * Official verifier smoke + negative tests using the staging-signed fixtures
 * shipped in the SDK repo (real 1Click staging signatures).
 */
import { describe, expect, it } from "vitest";
import { verifyQuoteSignature } from "@defuse-protocol/one-click-sdk-typescript";
import fixtures from "../../fixtures/one-click-staging-quotes.json" with { type: "json" };
import { WorkerError } from "../../src/errors.js";
import { QuoteResponseSchema, type QuoteRequest } from "../../src/near/types.js";
import { ONE_CLICK_STAGING_MANAGER_PUB_KEY, verifyQuoteResponse } from "../../src/near/verify.js";

const KEY = fixtures.stagingManagerPublicKey;

function parse(v: unknown) {
  const r = QuoteResponseSchema.safeParse(v);
  if (!r.success) throw new Error(JSON.stringify(r.error.issues));
  return r.data;
}

describe("official verifyQuoteSignature against staging fixtures", () => {
  it("staging key constant matches fixture file", () => {
    expect(ONE_CLICK_STAGING_MANAGER_PUB_KEY).toBe(KEY);
  });

  it("fixtures parse under our live-schema zod models", () => {
    expect(parse(fixtures.dryQuote).quoteRequest.dry).toBe(true);
    expect(parse(fixtures.nonDryQuote).quote.depositAddress).toBeTypeOf("string");
    expect(parse(fixtures.statusQuoteResponse).quoteRequest.confidentiality).toBe("public");
  });

  it("verifies dry, non-dry and status-embedded quotes", () => {
    expect(verifyQuoteSignature(fixtures.dryQuote as never, KEY)).toBe(true);
    expect(verifyQuoteSignature(fixtures.nonDryQuote as never, KEY)).toBe(true);
    expect(verifyQuoteSignature(fixtures.statusQuoteResponse as never, KEY)).toBe(true);
  });

  it("staging quotes do NOT verify under the production key (default)", () => {
    expect(verifyQuoteSignature(fixtures.nonDryQuote as never)).toBe(false);
  });

  it("rejects tampered depositAddress (MITM)", () => {
    const t = structuredClone(fixtures.nonDryQuote);
    t.quote.depositAddress = "bc1q0000000000000000000000000000000000000000";
    expect(verifyQuoteSignature(t as never, KEY)).toBe(false);
  });

  it("rejects tampered amountOut / minAmountOut / amountIn", () => {
    for (const field of ["amountOut", "minAmountOut", "amountIn", "minAmountIn"] as const) {
      const t = structuredClone(fixtures.nonDryQuote);
      t.quote[field] = (BigInt(t.quote[field]) + 1n).toString();
      expect(verifyQuoteSignature(t as never, KEY), field).toBe(false);
    }
  });

  it("rejects tampered recipient / refundTo / assets / amount / deadline", () => {
    const cases: [string, (q: typeof fixtures.nonDryQuote) => void][] = [
      ["recipient", (q) => (q.quoteRequest.recipient = "0x0000000000000000000000000000000000000001")],
      ["refundTo", (q) => (q.quoteRequest.refundTo = "bc1qattacker")],
      ["originAsset", (q) => (q.quoteRequest.originAsset = "nep141:eth.omft.near")],
      ["destinationAsset", (q) => (q.quoteRequest.destinationAsset = "nep141:wrap.near")],
      ["amount", (q) => (q.quoteRequest.amount = "10001")],
      ["quote.deadline", (q) => (q.quote.deadline = "2026-06-26T19:00:01.000Z")],
      ["timestamp", (q) => (q.timestamp = "2026-06-23T17:10:41.105Z")],
      ["swapType", (q) => (q.quoteRequest.swapType = "EXACT_OUTPUT")],
    ];
    for (const [name, mutate] of cases) {
      const t = structuredClone(fixtures.nonDryQuote);
      mutate(t);
      expect(verifyQuoteSignature(t as never, KEY), name).toBe(false);
    }
  });

  it("rejects missing, fake and malformed signatures", () => {
    for (const sig of ["", "not-a-signature", "ed25519:5fVqoCrPgqS9WPqnX5xvHKNYBqRZPkXvEqM9VaHZXgBbPYp7qZzx5HkNvZxQK1hBkD2qT8GJfXwR9nL4mS6vYt2"]) {
      const t = structuredClone(fixtures.nonDryQuote);
      t.signature = sig;
      expect(verifyQuoteSignature(t as never, KEY)).toBe(false);
    }
  });

  it("DOCUMENTED LIMITATION: on non-dry quotes quoteRequest.deadline is shadowed by quote.deadline in the signed payload", () => {
    // SDK 0.1.26 hashes stringify({...request, ...quote, timestamp}); `quote.deadline`
    // overwrites `quoteRequest.deadline` for executable quotes. Dry quotes have no
    // quote.deadline, so there the request deadline IS covered.
    const nonDry = structuredClone(fixtures.nonDryQuote);
    nonDry.quoteRequest.deadline = "2026-06-23T19:00:01.000Z";
    expect(verifyQuoteSignature(nonDry as never, KEY)).toBe(true); // not covered -> echo-binding check needed
    const dry = structuredClone(fixtures.dryQuote);
    dry.quoteRequest.deadline = "2026-06-23T19:00:01.000Z";
    expect(verifyQuoteSignature(dry as never, KEY)).toBe(false); // covered
  });

  it("DOCUMENTED LIMITATION: confidentiality is not covered by the signature", () => {
    const t = structuredClone(fixtures.nonDryQuote);
    t.quoteRequest.confidentiality = "basic"; // fixture was "public"
    // The official verifier still passes — this is why verify.ts adds echo-binding checks.
    expect(verifyQuoteSignature(t as never, KEY)).toBe(true);
  });
});

describe("verifyQuoteResponse (signature + echo binding)", () => {
  it("passes a genuine quote and throws SIGNATURE_INVALID on tamper", () => {
    const good = parse(fixtures.nonDryQuote);
    expect(() => verifyQuoteResponse(good, { managerPublicKey: KEY })).not.toThrow();
    const bad = parse(structuredClone(fixtures.nonDryQuote));
    bad.quote.depositAddress = "bc1q0000000000000000000000000000000000000000";
    expect(() => verifyQuoteResponse(bad, { managerPublicKey: KEY })).toThrowError(WorkerError);
    try {
      verifyQuoteResponse(bad, { managerPublicKey: KEY });
    } catch (e) {
      expect((e as WorkerError).code).toBe("SIGNATURE_INVALID");
    }
  });

  it("catches a confidentiality downgrade in the echoed request via echo binding", () => {
    const resp = parse(fixtures.nonDryQuote); // echoed confidentiality: public
    const sent = { ...resp.quoteRequest, confidentiality: "basic" as const } as unknown as QuoteRequest;
    expect(() => verifyQuoteResponse(resp, { managerPublicKey: KEY, sentRequest: sent })).toThrowError(/confidentiality/);
  });
});
