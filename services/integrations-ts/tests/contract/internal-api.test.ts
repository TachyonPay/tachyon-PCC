/**
 * Rust<->TS boundary tests: the worker's responses must satisfy the
 * Rust-owned OpenAPI contract, and the HTTP layer must enforce auth.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { AddressInfo } from "node:net";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { ERROR_CODES } from "../../src/errors.js";
import { NearService } from "../../src/near/service.js";
import { createApp } from "../../src/server/app.js";
import { FakeOneClickClient, USDC_ARB, USDC_BASE } from "../helpers/fakeClient.js";
import { makeTestSigner } from "../helpers/signer.js";

const openapi = JSON.parse(
  readFileSync(resolve(__dirname, "../../../../backend/crates/integration_client/openapi/internal-v1.json"), "utf8"),
) as {
  paths: Record<string, unknown>;
  components: { schemas: Record<string, { required?: string[]; properties?: Record<string, unknown>; enum?: string[] }> };
};

const TOKEN = "test-internal-token-0123456789";
let base = "";
let server: ReturnType<typeof createApp>;

beforeAll(async () => {
  const signer = makeTestSigner();
  const near = new NearService(new FakeOneClickClient(signer), { managerPublicKey: signer.publicKey });
  server = createApp({ internalToken: TOKEN, near, versions: { oneClickSdk: "test", zerodevSdk: "test" }, log: () => {} });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", () => r()));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => new Promise<void>((r) => server.close(() => r())));

/** `token: null` sends no Authorization header. */
const call = (path: string, init: RequestInit = {}, token: string | null = TOKEN) =>
  fetch(`${base}${path}`, {
    ...init,
    headers: { ...(init.headers as Record<string, string>), ...(token !== null ? { Authorization: `Bearer ${token}` } : {}), "Content-Type": "application/json" },
  });

function requiredOf(schema: string): string[] {
  return openapi.components.schemas[schema]?.required ?? [];
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const asJson = async (r: Response): Promise<any> => r.json();

describe("contract: schema/enum alignment", () => {
  it("every contract path is implemented by the worker", () => {
    const paths = Object.keys(openapi.paths);
    expect(paths.sort()).toEqual(
      [
        "/internal/v1/health",
        "/internal/v1/near/assets",
        "/internal/v1/near/quote",
        "/internal/v1/near/verify",
        "/internal/v1/near/status",
        "/internal/v1/near/deposit-submitted",
      ].sort(),
    );
  });

  it("ErrorCode enum matches src/errors.ts exactly", () => {
    expect([...(openapi.components.schemas["ErrorCode"]?.enum ?? [])].sort()).toEqual([...ERROR_CODES].sort());
  });

  it("OneClickStatus enum matches the live 1Click API", () => {
    expect(openapi.components.schemas["OneClickStatus"]?.enum).toEqual([
      "KNOWN_DEPOSIT_TX",
      "PENDING_DEPOSIT",
      "INCOMPLETE_DEPOSIT",
      "PROCESSING",
      "SUCCESS",
      "REFUNDED",
      "FAILED",
    ]);
  });
});

describe("contract: HTTP layer", () => {
  it("rejects missing / wrong bearer tokens with the contract error body", async () => {
    for (const t of [null, "", "wrong", TOKEN + "x", TOKEN.slice(0, -1)]) {
      const r = await call("/internal/v1/health", {}, t);
      expect(r.status).toBe(401);
      const body = await asJson(r);
      expect(Object.keys(body)).toEqual(["error"]);
      expect(body.error.code).toBe("UNAUTHORIZED");
      for (const k of requiredOf("ErrorBody")) expect(body.error).toHaveProperty(k);
    }
  });

  it("health satisfies HealthResponse", async () => {
    const r = await call("/internal/v1/health");
    expect(r.status).toBe(200);
    const body = await asJson(r);
    for (const k of requiredOf("HealthResponse")) expect(body, k).toHaveProperty(k);
  });

  it("assets satisfies AssetsResponse / TokenEntry", async () => {
    const r = await call("/internal/v1/near/assets");
    const body = await asJson(r);
    for (const k of requiredOf("AssetsResponse")) expect(body).toHaveProperty(k);
    for (const k of requiredOf("TokenEntry")) expect(body.tokens[0]).toHaveProperty(k);
  });

  it("quote satisfies VerifiedQuoteDto and amounts are strings", async () => {
    const r = await call("/internal/v1/near/quote", {
      method: "POST",
      body: JSON.stringify({
        dry: false,
        authorizationRef: "contract-test-ref-1",
        originAssetId: USDC_BASE,
        destinationAssetId: USDC_ARB,
        amountOut: "1000000",
        recipient: "0x1111111111111111111111111111111111111111",
        refundTo: "0x2222222222222222222222222222222222222222",
        confidentiality: "basic",
        slippageBps: 100,
        deadline: new Date(Date.now() + 3600_000).toISOString(),
      }),
    });
    expect(r.status).toBe(200);
    const body = await asJson(r);
    for (const k of requiredOf("VerifiedQuoteDto")) expect(body, k).toHaveProperty(k);
    for (const k of ["amountIn", "minAmountIn", "amountOut", "minAmountOut"]) expect(typeof body[k]).toBe("string");
    // No property outside the contract leaks out.
    const allowed = new Set(Object.keys(openapi.components.schemas["VerifiedQuoteDto"]?.properties ?? {}));
    for (const k of Object.keys(body)) expect(allowed.has(k), `unexpected property ${k}`).toBe(true);
  });

  it("invalid body -> 400 INVALID_REQUEST; reused ref -> 409 AUTH_REF_REUSED", async () => {
    const r1 = await call("/internal/v1/near/quote", { method: "POST", body: "{not json" });
    expect(r1.status).toBe(400);
    expect((await asJson(r1)).error.code).toBe("INVALID_REQUEST");

    const payload = JSON.stringify({
      dry: false,
      authorizationRef: "contract-test-ref-2",
      originAssetId: USDC_BASE,
      destinationAssetId: USDC_ARB,
      amountOut: "1000000",
      recipient: "0x1111111111111111111111111111111111111111",
      refundTo: "0x2222222222222222222222222222222222222222",
      confidentiality: "basic",
      slippageBps: 100,
      deadline: new Date(Date.now() + 3600_000).toISOString(),
    });
    expect((await call("/internal/v1/near/quote", { method: "POST", body: payload })).status).toBe(200);
    const dup = await call("/internal/v1/near/quote", { method: "POST", body: payload });
    expect(dup.status).toBe(409);
    expect((await asJson(dup)).error.code).toBe("AUTH_REF_REUSED");
  });

  it("unknown route -> 404 with contract error body; oversized body rejected", async () => {
    const r = await call("/internal/v1/near/sign-anything", { method: "POST", body: "{}" });
    expect(r.status).toBe(404);
    expect((await asJson(r)).error.code).toBe("INVALID_REQUEST");
    const big = await call("/internal/v1/near/verify", { method: "POST", body: JSON.stringify({ rawResponseJson: "x".repeat(300 * 1024) }) });
    expect(big.status).toBe(400);
  });
});
