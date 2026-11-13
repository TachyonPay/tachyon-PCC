/**
 * Per-payer spending allowances. The payer signs the enable payload once with
 * their own key; the worker only verifies it, stores the result, and counts
 * executed transfers against the signed number of uses.
 */
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { Address, TypedDataDefinition } from "viem";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";
import { AaService, type DepositPlanInput } from "../../src/account-abstraction/service.js";
import {
  GrantError,
  MAX_ALLOWANCE_SECONDS,
  MAX_ALLOWANCE_USES,
  SessionGrantService,
  toJsonSafe,
  type AllowanceRequest,
} from "../../src/account-abstraction/session_grants.js";
import { CHECKOUT_MAX_GAS_WEI, SUPPORTED_7702_CHAINS } from "../../src/account-abstraction/pins.js";
import { FakeGrantBackend } from "../helpers/grants.js";

const TOKEN = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" as Address;
const OTHER_TOKEN = "0xfde4C96c8593536E31F229EA8f37b2ADa2699bb2" as Address;
const DEPOSIT = "0x76b4c56085ED136a8744D52bE956396624a730E8" as Address;
const CHAIN = SUPPORTED_7702_CHAINS.sepolia;
const NOW = 1_800_000_000;
const WEEK = 7 * 24 * 3600;

function setup(now = () => NOW) {
  const backend = new FakeGrantBackend();
  const grants = new SessionGrantService(backend, { now });
  const payer = privateKeyToAccount(generatePrivateKey());
  return { backend, grants, payer };
}

function aaWith(grants: SessionGrantService, now = () => NOW) {
  return AaService.withMemory({
    ledgerDir: mkdtempSync(join(tmpdir(), "pcc-grant-")),
    gates: { dryRun: false, enableMainnetTransfers: false },
    now,
    grants,
  });
}

let attemptSeq = 0;
function plan(from: Address, over: Partial<DepositPlanInput> = {}): DepositPlanInput {
  attemptSeq += 1;
  return {
    attemptId: `00000000-0000-0000-0000-${attemptSeq.toString().padStart(12, "0")}`,
    chainId: CHAIN,
    token: TOKEN,
    from,
    to: DEPOSIT,
    quoteDepositAddress: DEPOSIT,
    amount: 1_000_000n,
    maxFeeWei: 300_000_000_000_000n,
    approvalRef: "operator:test",
    ...over,
  };
}

async function signPrepared(
  grants: SessionGrantService,
  signer: ReturnType<typeof privateKeyToAccount>,
  over: Partial<AllowanceRequest> = {},
) {
  const prepared = await grants.prepare({
    owner: signer.address,
    chainId: CHAIN,
    token: TOKEN,
    maxAmount: 1_000_000n,
    uses: 3,
    validitySec: WEEK,
    ...over,
  });
  // Round-trip through JSON exactly as the browser receives it.
  const wire = JSON.parse(JSON.stringify(prepared.typedData)) as TypedDataDefinition;
  const signature = await signer.signTypedData(wire);
  return { prepared, signature };
}

describe("spending allowances", () => {
  it("one signature covers the signed number of capped transfers, then stops", async () => {
    const { backend, grants, payer } = setup();
    const { prepared, signature } = await signPrepared(grants, payer);
    expect(prepared.sessionSigner).not.toBe(payer.address);
    expect(prepared.uses).toBe(3);
    expect(prepared.validUntil).toBe(NOW + WEEK);
    const message = (prepared.typedData as { message: Record<string, unknown> }).message;
    expect(message.maxAmount).toBe("1000000");
    expect(message.uses).toBe(3);
    expect(message.gasBudget).toBe((CHECKOUT_MAX_GAS_WEI * 3n).toString());

    const enabled = await grants.enable(prepared.grantId, signature);
    expect(enabled).toMatchObject({ owner: payer.address, maxAmount: 1_000_000n, usesRemaining: 3, validUntil: NOW + WEEK });
    expect(backend.serialized).toHaveLength(1);
    expect(backend.serialized[0]!.spec.grant.rateLimitCount).toBe(3);

    const aa = aaWith(grants);
    expect((await aa.simulate(plan(payer.address, { amount: 1_000_001n }))).rejectCode).toBe("excessive_amount");
    for (const left of [2, 1]) {
      expect((await aa.simulate(plan(payer.address))).ok).toBe(true);
      await aa.execute(plan(payer.address));
      expect(grants.status(CHAIN, payer.address, TOKEN)?.usesRemaining).toBe(left);
    }
    await aa.execute(plan(payer.address));
    expect(grants.status(CHAIN, payer.address, TOKEN)).toBeUndefined();
    expect((await aa.simulate(plan(payer.address))).rejectCode).toBe("no_session");
    expect(backend.serialized).toHaveLength(1);
  });

  it("re-executing the same attempt does not spend another use", async () => {
    const { grants, payer } = setup();
    const { prepared, signature } = await signPrepared(grants, payer);
    await grants.enable(prepared.grantId, signature);
    const aa = aaWith(grants);
    const once = plan(payer.address);
    await aa.execute(once);
    await aa.execute(once);
    expect(grants.status(CHAIN, payer.address, TOKEN)?.usesRemaining).toBe(2);
  });

  it("is scoped to one payer, chain and token", async () => {
    const { grants, payer } = setup();
    const other = privateKeyToAccount(generatePrivateKey());
    const { prepared, signature } = await signPrepared(grants, payer);
    await grants.enable(prepared.grantId, signature);
    const aa = aaWith(grants);
    expect((await aa.simulate(plan(other.address))).rejectCode).toBe("no_session");
    expect((await aa.simulate(plan(payer.address, { token: OTHER_TOKEN }))).ok).toBe(false);
    expect(grants.status(CHAIN, payer.address, OTHER_TOKEN)).toBeUndefined();
    expect(grants.status(CHAIN, payer.address, TOKEN)?.usesRemaining).toBe(3);
  });

  it("an autopay mandate has its own allowance beside the checkout one", async () => {
    const { grants, payer } = setup();
    const mandateId = "33333333-3333-4333-8333-333333333333";
    const checkout = await signPrepared(grants, payer, { uses: 2 });
    await grants.enable(checkout.prepared.grantId, checkout.signature);
    const mandate = await signPrepared(grants, payer, { uses: 4, maxAmount: 500_000n, mandateId });
    const enabled = await grants.enable(mandate.prepared.grantId, mandate.signature);
    expect(enabled).toMatchObject({ mandateId, usesRemaining: 4, maxAmount: 500_000n });
    expect(grants.status(CHAIN, payer.address, TOKEN)?.usesRemaining).toBe(2);

    const aa = aaWith(grants);
    expect((await aa.simulate(plan(payer.address, { mandateId, amount: 500_001n }))).rejectCode).toBe("excessive_amount");
    expect((await aa.simulate(plan(payer.address, { amount: 1_000_000n }))).ok).toBe(true);
    await aa.execute(plan(payer.address, { mandateId, amount: 500_000n }));
    expect(grants.status(CHAIN, payer.address, TOKEN, mandateId)?.usesRemaining).toBe(3);
    expect(grants.status(CHAIN, payer.address, TOKEN)?.usesRemaining).toBe(2);

    expect(grants.revoke(CHAIN, payer.address, TOKEN, mandateId)).toBe(true);
    expect((await aa.simulate(plan(payer.address, { mandateId, amount: 1n }))).rejectCode).toBe("no_session");
    expect(grants.status(CHAIN, payer.address, TOKEN)?.usesRemaining).toBe(2);
    expect(grants.status(CHAIN, payer.address, TOKEN, "44444444-4444-4444-8444-444444444444")).toBeUndefined();
  });

  it("expires at validUntil even with uses left", async () => {
    let now = NOW;
    const { grants, payer } = setup(() => now);
    const { prepared, signature } = await signPrepared(grants, payer, { validitySec: 3600 });
    await grants.enable(prepared.grantId, signature);
    now = NOW + 3599;
    expect(grants.status(CHAIN, payer.address, TOKEN)?.usesRemaining).toBe(3);
    now = NOW + 3600;
    expect(grants.status(CHAIN, payer.address, TOKEN)).toBeUndefined();
  });

  it("revoke destroys the session key immediately", async () => {
    const { grants, payer } = setup();
    const { prepared, signature } = await signPrepared(grants, payer);
    await grants.enable(prepared.grantId, signature);
    expect(grants.revoke(CHAIN, payer.address, TOKEN)).toBe(true);
    expect(grants.activeFor(CHAIN, payer.address, TOKEN)).toBeUndefined();
    expect((await aaWith(grants).simulate(plan(payer.address))).rejectCode).toBe("no_session");
    expect(grants.revoke(CHAIN, payer.address, TOKEN)).toBe(false);
  });

  it("a new allowance replaces the old one for the same token", async () => {
    const { grants, payer } = setup();
    const first = await signPrepared(grants, payer, { uses: 2 });
    await grants.enable(first.prepared.grantId, first.signature);
    const second = await signPrepared(grants, payer, { uses: 5, maxAmount: 2_000_000n });
    await grants.enable(second.prepared.grantId, second.signature);
    expect(grants.status(CHAIN, payer.address, TOKEN)).toMatchObject({ usesRemaining: 5, maxAmount: 2_000_000n });
    expect(grants.activeFor(CHAIN, payer.address, TOKEN)?.grant.sessionSigner).toBe(second.prepared.sessionSigner);
  });

  it("bounds uses and validity", async () => {
    const { grants, payer } = setup();
    const base = { owner: payer.address, chainId: CHAIN, token: TOKEN, maxAmount: 1n, uses: 1, validitySec: 3600 };
    for (const bad of [
      { uses: 0 },
      { uses: MAX_ALLOWANCE_USES + 1 },
      { uses: 1.5 },
      { validitySec: 599 },
      { validitySec: MAX_ALLOWANCE_SECONDS + 1 },
      { maxAmount: 0n },
    ]) {
      await expect(grants.prepare({ ...base, ...bad })).rejects.toMatchObject({ code: "invalid_allowance" });
    }
    await expect(grants.prepare({ ...base, uses: MAX_ALLOWANCE_USES, validitySec: MAX_ALLOWANCE_SECONDS })).resolves.toBeDefined();
  });

  it("refuses a signature from anyone but the payer", async () => {
    const { backend, grants, payer } = setup();
    const thief = privateKeyToAccount(generatePrivateKey());
    const { prepared } = await signPrepared(grants, payer);
    const forged = await thief.signTypedData(JSON.parse(JSON.stringify(prepared.typedData)) as TypedDataDefinition);
    await expect(grants.enable(prepared.grantId, forged)).rejects.toMatchObject({ code: "signature_invalid" });
    await expect(grants.enable(prepared.grantId, "0xdead")).rejects.toBeInstanceOf(GrantError);
    expect(backend.serialized).toHaveLength(0);
    expect(grants.activeFor(CHAIN, payer.address, TOKEN)).toBeUndefined();
  });

  it("refuses a signature over a different payload (more uses or a bigger cap)", async () => {
    const { grants, payer } = setup();
    const { prepared } = await signPrepared(grants, payer);
    const wire = JSON.parse(JSON.stringify(prepared.typedData)) as TypedDataDefinition & { message: Record<string, unknown> };
    for (const change of [{ maxAmount: "999999999" }, { uses: 20 }]) {
      const tampered = { ...wire, message: { ...wire.message, ...change } } as TypedDataDefinition;
      const sig = await payer.signTypedData(tampered);
      await expect(grants.enable(prepared.grantId, sig)).rejects.toMatchObject({ code: "signature_invalid" });
    }
  });

  it("unknown, reused and late enables are refused", async () => {
    let now = NOW;
    const { grants, payer } = setup(() => now);
    await expect(grants.enable("nope", "0x00")).rejects.toMatchObject({ code: "unknown_grant" });

    const { prepared, signature } = await signPrepared(grants, payer);
    await grants.enable(prepared.grantId, signature);
    await expect(grants.enable(prepared.grantId, signature)).rejects.toMatchObject({ code: "unknown_grant" });

    const late = await signPrepared(grants, payer);
    now += 601;
    await expect(grants.enable(late.prepared.grantId, late.signature)).rejects.toMatchObject({ code: "grant_expired" });
  });

  it("refuses to prepare for an undelegated owner", async () => {
    const { backend, grants, payer } = setup();
    backend.delegated = false;
    await expect(
      grants.prepare({ owner: payer.address, chainId: CHAIN, token: TOKEN, maxAmount: 1n, uses: 1, validitySec: 3600 }),
    ).rejects.toThrow(/not_delegated/);
  });

  it("toJsonSafe stringifies nested bigints only", () => {
    expect(toJsonSafe({ a: 1n, b: [2n, "x"], c: { d: 3 } })).toEqual({ a: "1", b: ["2", "x"], c: { d: 3 } });
  });
});
