/**
 * Encrypted allowance store: a worker restart restores session keys, and a
 * wrong key or a modified file fails closed instead of starting empty.
 */
import { mkdtempSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { Address, TypedDataDefinition } from "viem";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";
import { EncryptedGrantStore, GrantStoreError, MIN_STORE_KEY_LENGTH } from "../../src/account-abstraction/grant_store.js";
import { SessionGrantService, type AllowanceRequest } from "../../src/account-abstraction/session_grants.js";
import { SUPPORTED_7702_CHAINS } from "../../src/account-abstraction/pins.js";
import { FakeGrantBackend } from "../helpers/grants.js";

const TOKEN = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" as Address;
const CHAIN = SUPPORTED_7702_CHAINS.sepolia;
const NOW = 1_800_000_000;
const WEEK = 7 * 24 * 3600;
const KEY = "a".repeat(MIN_STORE_KEY_LENGTH);

function dir(): string {
  return mkdtempSync(join(tmpdir(), "pcc-grant-store-"));
}

async function enable(grants: SessionGrantService, over: Partial<AllowanceRequest> = {}) {
  const payer = privateKeyToAccount(generatePrivateKey());
  const prepared = await grants.prepare({
    owner: payer.address,
    chainId: CHAIN,
    token: TOKEN,
    maxAmount: 1_000_000n,
    uses: 3,
    validitySec: WEEK,
    ...over,
  });
  const wire = JSON.parse(JSON.stringify(prepared.typedData)) as TypedDataDefinition;
  await grants.enable(prepared.grantId, await payer.signTypedData(wire));
  return payer;
}

describe("encrypted allowance store", () => {
  it("restores an allowance, its session key and its remaining uses after a restart", async () => {
    const path = dir();
    const store = new EncryptedGrantStore(path, KEY);
    const grants = new SessionGrantService(new FakeGrantBackend(), { now: () => NOW, store });
    const payer = await enable(grants);
    const before = grants.activeFor(CHAIN, payer.address, TOKEN);
    expect(before?.usesRemaining).toBe(3);
    grants.consume(CHAIN, payer.address, TOKEN, "attempt-1");

    const restarted = new SessionGrantService(new FakeGrantBackend(), { now: () => NOW, store: new EncryptedGrantStore(path, KEY) });
    const after = restarted.activeFor(CHAIN, payer.address, TOKEN);
    expect(after?.sessionKey).toBe(before?.sessionKey);
    expect(after?.serialized).toBe(before?.serialized);
    expect(after?.usesRemaining).toBe(2);
    expect(after?.consumedAttempts.has("attempt-1")).toBe(true);
    expect(statSync(join(path, "grants.v1.enc")).mode & 0o777).toBe(0o600);

    restarted.revoke(CHAIN, payer.address, TOKEN);
    const empty = new SessionGrantService(new FakeGrantBackend(), { now: () => NOW, store: new EncryptedGrantStore(path, KEY) });
    expect(empty.size).toBe(0);
  });

  it("keeps a mandate allowance separate from the checkout allowance", async () => {
    const path = dir();
    const mandateId = "33333333-3333-4333-8333-333333333333";
    const grants = new SessionGrantService(new FakeGrantBackend(), { now: () => NOW, store: new EncryptedGrantStore(path, KEY) });
    const payer = await enable(grants, { mandateId, uses: 2 });
    const restarted = new SessionGrantService(new FakeGrantBackend(), { now: () => NOW, store: new EncryptedGrantStore(path, KEY) });
    expect(restarted.status(CHAIN, payer.address, TOKEN)).toBeUndefined();
    expect(restarted.status(CHAIN, payer.address, TOKEN, mandateId)?.usesRemaining).toBe(2);
  });

  it("drops an expired allowance on load instead of restoring it", async () => {
    const path = dir();
    const store = new EncryptedGrantStore(path, KEY);
    const grants = new SessionGrantService(new FakeGrantBackend(), { now: () => NOW, store });
    const payer = await enable(grants);
    const later = new SessionGrantService(new FakeGrantBackend(), {
      now: () => NOW + WEEK,
      store: new EncryptedGrantStore(path, KEY),
    });
    expect(later.activeFor(CHAIN, payer.address, TOKEN)).toBeUndefined();
    expect(later.size).toBe(0);
  });

  it("fails closed on a short key, a wrong key, and a modified file", async () => {
    expect(() => new EncryptedGrantStore(dir(), "too-short")).toThrow(GrantStoreError);

    const path = dir();
    const grants = new SessionGrantService(new FakeGrantBackend(), { now: () => NOW, store: new EncryptedGrantStore(path, KEY) });
    await enable(grants);
    expect(() => new EncryptedGrantStore(path, `${"b".repeat(MIN_STORE_KEY_LENGTH)}`).load()).toThrow(/wrong GRANT_STORE_KEY/);

    const file = join(path, "grants.v1.enc");
    const env = JSON.parse(readFileSync(file, "utf8")) as { ct: string };
    env.ct = `${env.ct.slice(0, -4)}AAAA`;
    writeFileSync(file, JSON.stringify(env));
    expect(() => new EncryptedGrantStore(path, KEY).load()).toThrow(/modified file/);
  });
});
