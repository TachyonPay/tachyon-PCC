/**
 * Rust<->TS boundary tests: the worker's responses must satisfy the
 * Rust-owned OpenAPI contract, and the HTTP layer must enforce auth.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { AddressInfo } from "node:net";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Address } from "viem";
import { AaService } from "../../src/account-abstraction/service.js";
import { buildGrant } from "../../src/account-abstraction/grant.js";
import { SUPPORTED_7702_CHAINS } from "../../src/account-abstraction/pins.js";
import { ERROR_CODES } from "../../src/errors.js";
import { NearService } from "../../src/near/service.js";
import { createApp } from "../../src/server/app.js";
import { FakeOneClickClient, USDC_ARB, USDC_BASE } from "../helpers/fakeClient.js";
import { makeTestSigner } from "../helpers/signer.js";
import { FakeGrantBackend } from "../helpers/grants.js";
import { SessionGrantService } from "../../src/account-abstraction/session_grants.js";
import type { TypedDataDefinition } from "viem";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";

const openapi = JSON.parse(
  readFileSync(resolve(__dirname, "../../../../backend/crates/integration_client/openapi/internal-v1.json"), "utf8"),
) as {
  paths: Record<string, unknown>;
  components: { schemas: Record<string, { required?: string[]; properties?: Record<string, unknown>; enum?: string[] }> };
};

const TOKEN = "test-internal-token-0123456789";
let base = "";
let server: ReturnType<typeof createApp>;

const OWNER = "0x049FdEf2550772B9F36F86757C27f5F982D0970D" as Address;
const SESSION = "0x06e6d59dd649550767405A2E3B1CC13fABE550E7" as Address;
const TOKEN_USDC = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" as Address;
const DEPOSIT = "0x76b4c56085ED136a8744D52bE956396624a730E8" as Address;
const NOW = 1_800_000_000;

beforeAll(async () => {
  const signer = makeTestSigner();
  const near = new NearService(new FakeOneClickClient(signer), { managerPublicKey: signer.publicKey });
  const aa = AaService.withMemory({
    ledgerDir: mkdtempSync(join(tmpdir(), "pcc-aa-http-")),
    gates: { dryRun: false, enableMainnetTransfers: false },
    now: () => NOW,
    grants: new SessionGrantService(new FakeGrantBackend(), { now: () => NOW }),
  });
  aa.enroll(
    buildGrant({
      owner: OWNER,
      sessionSigner: SESSION,
      chainId: SUPPORTED_7702_CHAINS.sepolia,
      token: TOKEN_USDC,
      maxAmount: 2_000_000n,
      validUntil: NOW + 3600,
      nowUnix: NOW,
    }),
  );
  server = createApp({
    internalToken: TOKEN,
    near,
    versions: { oneClickSdk: "test", zerodevSdk: "test" },
    aa,
    log: () => {},
  });
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
        "/internal/v1/aa/simulate",
        "/internal/v1/aa/execute",
        "/internal/v1/aa/receipt",
        "/internal/v1/aa/grants/prepare",
        "/internal/v1/aa/grants/enable",
        "/internal/v1/aa/grants/status",
        "/internal/v1/aa/grants/revoke",
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

  it("aa simulate/execute bind the quote deposit and refuse substitution", async () => {
    const payload = {
      attemptId: "11111111-1111-1111-1111-111111111111",
      chainId: SUPPORTED_7702_CHAINS.sepolia,
      token: TOKEN_USDC,
      from: OWNER,
      to: DEPOSIT,
      quoteDepositAddress: DEPOSIT,
      amount: "1020000",
      maxFeeWei: "10000000000000000",
      approvalRef: "appr-contract-aa",
    };
    const sim = await call("/internal/v1/aa/simulate", { method: "POST", body: JSON.stringify(payload) });
    expect(sim.status).toBe(200);
    const simBody = await asJson(sim);
    expect(simBody.ok).toBe(true);
    for (const k of requiredOf("SimulationDto")) expect(simBody).toHaveProperty(k);

    const exec = await call("/internal/v1/aa/execute", { method: "POST", body: JSON.stringify(payload) });
    expect(exec.status).toBe(200);
    const execBody = await asJson(exec);
    for (const k of requiredOf("ExecuteResponseDto")) expect(execBody).toHaveProperty(k);
    expect(execBody.userOpHash).toMatch(/^0x[0-9a-f]{64}$/);

    const rec = await call(
      `/internal/v1/aa/receipt?chainId=${SUPPORTED_7702_CHAINS.sepolia}&txHash=${execBody.txHash}`,
    );
    expect(rec.status).toBe(200);
    expect((await asJson(rec)).success).toBe(true);

    const bad = await call("/internal/v1/aa/execute", {
      method: "POST",
      body: JSON.stringify({
        ...payload,
        attemptId: "22222222-2222-2222-2222-222222222222",
        to: "0x3333333333333333333333333333333333333333",
      }),
    });
    expect(bad.status).toBe(422);
    expect((await asJson(bad)).error.code).toBe("SESSION_REJECTED");
  });

  it("aa grants: prepare -> payer signs -> enable -> status -> revoke, matching the contract", async () => {
    const payer = privateKeyToAccount(generatePrivateKey());
    const key = { owner: payer.address, chainId: SUPPORTED_7702_CHAINS.sepolia, token: TOKEN_USDC };
    const prep = await call("/internal/v1/aa/grants/prepare", {
      method: "POST",
      body: JSON.stringify({ ...key, maxAmount: "1500000", uses: 3, validitySec: 7 * 24 * 3600 }),
    });
    expect(prep.status).toBe(200);
    const prepared = await asJson(prep);
    for (const k of requiredOf("GrantPreparedDto")) expect(prepared).toHaveProperty(k);

    const forged = await privateKeyToAccount(generatePrivateKey()).signTypedData(prepared.typedData as TypedDataDefinition);
    const bad = await call("/internal/v1/aa/grants/enable", {
      method: "POST",
      body: JSON.stringify({ grantId: prepared.grantId, signature: forged }),
    });
    expect(bad.status).toBe(422);
    const badBody = await asJson(bad);
    expect(badBody.error.code).toBe("SESSION_REJECTED");
    expect(badBody.error.message).toMatch(/signature_invalid/);

    const signature = await payer.signTypedData(prepared.typedData as TypedDataDefinition);
    const ok = await call("/internal/v1/aa/grants/enable", {
      method: "POST",
      body: JSON.stringify({ grantId: prepared.grantId, signature }),
    });
    expect(ok.status).toBe(200);
    const enabled = await asJson(ok);
    for (const k of requiredOf("AllowanceDto")) expect(enabled).toHaveProperty(k);
    expect(enabled.owner).toBe(payer.address);
    expect(enabled.maxAmount).toBe("1500000");
    expect(enabled.usesRemaining).toBe(3);

    const status = await call("/internal/v1/aa/grants/status", { method: "POST", body: JSON.stringify(key) });
    expect(status.status).toBe(200);
    const live = await asJson(status);
    for (const k of requiredOf("AllowanceDto")) expect(live).toHaveProperty(k);
    expect(live.usesRemaining).toBe(3);

    const revoke = () => call("/internal/v1/aa/grants/revoke", { method: "POST", body: JSON.stringify(key) });
    const first = await revoke();
    expect(first.status).toBe(200);
    expect(await asJson(first)).toEqual({ revoked: true });
    expect(await asJson(await revoke())).toEqual({ revoked: false });
    const gone = await call("/internal/v1/aa/grants/status", { method: "POST", body: JSON.stringify(key) });
    expect(gone.status).toBe(404);

    const mandateId = "33333333-3333-4333-8333-333333333333";
    const mPrep = await call("/internal/v1/aa/grants/prepare", {
      method: "POST",
      body: JSON.stringify({ ...key, mandateId, maxAmount: "700000", uses: 2, validitySec: 3600 }),
    });
    expect(mPrep.status).toBe(200);
    const mPrepared = await asJson(mPrep);
    const mSig = await payer.signTypedData(mPrepared.typedData as TypedDataDefinition);
    const mOk = await call("/internal/v1/aa/grants/enable", {
      method: "POST",
      body: JSON.stringify({ grantId: mPrepared.grantId, signature: mSig }),
    });
    expect(await asJson(mOk)).toMatchObject({ mandateId, usesRemaining: 2, maxAmount: "700000" });
    const mStatus = await call("/internal/v1/aa/grants/status", { method: "POST", body: JSON.stringify({ ...key, mandateId }) });
    expect((await asJson(mStatus)).mandateId).toBe(mandateId);
    const checkoutGone = await call("/internal/v1/aa/grants/status", { method: "POST", body: JSON.stringify(key) });
    expect(checkoutGone.status, "a mandate does not resurrect the checkout allowance").toBe(404);
    const badMandate = await call("/internal/v1/aa/grants/status", {
      method: "POST",
      body: JSON.stringify({ ...key, mandateId: "not-a-uuid" }),
    });
    expect(badMandate.status).toBe(400);

    for (const [body, status] of [
      [{ owner: "nope", chainId: 1, token: TOKEN_USDC, maxAmount: "1", uses: 1, validitySec: 600 }, 400],
      [{ ...key, maxAmount: "1" }, 400],
      [{ ...key, maxAmount: "1", uses: 21, validitySec: 600 }, 422],
      [{ ...key, maxAmount: "1", uses: 1, validitySec: 599 }, 422],
    ] as const) {
      const res = await call("/internal/v1/aa/grants/prepare", { method: "POST", body: JSON.stringify(body) });
      expect(res.status, JSON.stringify(body)).toBe(status);
    }
  });
});
