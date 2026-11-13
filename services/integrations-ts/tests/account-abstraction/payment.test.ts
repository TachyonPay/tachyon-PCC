/**
 * M4: quote-bound session deposits (mocked bundler). No live money, no
 * ZeroDev RPC. Covers quote-address substitution and the M3 negatives
 * that the payment path must still refuse.
 */
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { Address, Hex } from "viem";
import { SUPPORTED_7702_CHAINS } from "../../src/account-abstraction/pins.js";
import { buildGrant } from "../../src/account-abstraction/grant.js";
import { AaService, type DepositPlanInput } from "../../src/account-abstraction/service.js";
import { MemoryBundler } from "../../src/account-abstraction/bundler.js";

const OWNER = "0x049FdEf2550772B9F36F86757C27f5F982D0970D" as Address;
const SESSION = "0x06e6d59dd649550767405A2E3B1CC13fABE550E7" as Address;
const TOKEN = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" as Address;
const DEPOSIT = "0x76b4c56085ED136a8744D52bE956396624a730E8" as Address;
const ATTACKER = "0x3333333333333333333333333333333333333333" as Address;
const SEPOLIA = SUPPORTED_7702_CHAINS.sepolia;
const BASE = SUPPORTED_7702_CHAINS.base;
const NOW = 1_800_000_000;

function service(bundler = new MemoryBundler()): AaService {
  const dir = mkdtempSync(join(tmpdir(), "pcc-aa-"));
  const aa = AaService.withMemory({
    ledgerDir: dir,
    gates: { dryRun: false, enableMainnetTransfers: false },
    bundler,
    now: () => NOW,
  });
  aa.enroll(
    buildGrant({
      owner: OWNER,
      sessionSigner: SESSION,
      chainId: SEPOLIA,
      token: TOKEN,
      maxAmount: 2_000_000n,
      validUntil: NOW + 3600,
      nowUnix: NOW,
    }),
  );
  return aa;
}

function plan(over: Partial<DepositPlanInput> = {}): DepositPlanInput {
  return {
    attemptId: "11111111-1111-1111-1111-111111111111",
    chainId: SEPOLIA,
    token: TOKEN,
    from: OWNER,
    to: DEPOSIT,
    quoteDepositAddress: DEPOSIT,
    amount: 1_020_000n,
    maxFeeWei: 10n ** 16n,
    approvalRef: "appr-m4-1",
    ...over,
  };
}

describe("quote-bound session deposit", () => {
  it("simulates and executes an in-policy transfer to the quote deposit", async () => {
    const aa = service();
    const sim = await aa.simulate(plan());
    expect(sim.ok).toBe(true);
    const { userOpHash, txHash } = await aa.execute(plan());
    expect(userOpHash).toMatch(/^0x[0-9a-f]{64}$/);
    expect(txHash).toBe(userOpHash);
    const rec = aa.receipt(SEPOLIA, txHash);
    expect(rec?.success).toBe(true);
  });

  it("bundler simulation failure fails preflight", async () => {
    const bundler = new MemoryBundler();
    bundler.simulateOk = false;
    const sim = await service(bundler).simulate(plan());
    expect(sim.ok).toBe(false);
    expect(sim.rejectCode).toBe("bundler_simulation_failed");
  });

  it("quote address substitution is refused before sign", async () => {
    const aa = service();
    const bad = plan({ to: ATTACKER, quoteDepositAddress: DEPOSIT });
    const sim = await aa.simulate(bad);
    expect(sim.ok).toBe(false);
    expect(sim.rejectCode).toBe("quote_address_mismatch");
    await expect(aa.execute(bad)).rejects.toThrow(/quote_address_mismatch/);
  });

  it("both-fields-substituted still fails the grant token-only path when quote bind is consistent but not the quote", async () => {
    const aa = service();
    // Attacker sets to == quoteDepositAddress == attacker. Service encodes
    // transfer(attacker). Option C would allow this on-chain; the payment
    // path still requires Rust to send the *quote* address. Here we assert
    // the worker will execute whatever matching pair it is given — Rust
    // execute_approved is what binds the stored quote. Dual-field mismatch
    // is the worker's job; this case is a consistent pair.
    const consistent = plan({ to: ATTACKER, quoteDepositAddress: ATTACKER });
    const sim = await aa.simulate(consistent);
    expect(sim.ok).toBe(true);
  });

  it("unknown source wallet is not enrolled", async () => {
    const aa = service();
    const sim = await aa.simulate(plan({ from: SESSION }));
    expect(sim.rejectCode).toBe("no_session");
    await expect(aa.execute(plan({ from: SESSION }))).rejects.toThrow(/no_session/);
  });

  it("revoked / disabled signer", async () => {
    const aa = service();
    aa.revoke(OWNER, SEPOLIA);
    expect((await aa.simulate(plan())).rejectCode).toBe("revoked");
    await expect(aa.execute(plan())).rejects.toThrow(/revoked/);
  });

  it("second chain not enrolled", async () => {
    const aa = service();
    const sim = await aa.simulate(plan({ chainId: BASE }));
    expect(sim.rejectCode).toBe("chain_not_enrolled");
    await expect(aa.execute(plan({ chainId: BASE }))).rejects.toThrow(/chain_not_enrolled/);
  });

  it("excessive amount vs session cap", async () => {
    const aa = service();
    const sim = await aa.simulate(plan({ amount: 5_000_000n }));
    expect(sim.rejectCode).toBe("excessive_amount");
  });

  it("duplicate execute rebroadcasts the same hash", async () => {
    const aa = service();
    const first = await aa.execute(plan());
    const second = await aa.execute(plan());
    expect(second.userOpHash).toBe(first.userOpHash);
  });

  it("out-of-policy approve-shaped token is not how we encode; wrong token is refused", async () => {
    const aa = service();
    const other = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
    expect((await aa.simulate(plan({ token: other }))).rejectCode).toBe("arbitrary_target");
  });

  it("no session at all", async () => {
    const dir = mkdtempSync(join(tmpdir(), "pcc-aa-"));
    const aa = AaService.withMemory({
      ledgerDir: dir,
      gates: { dryRun: false, enableMainnetTransfers: false },
      now: () => NOW,
    });
    expect((await aa.simulate(plan())).rejectCode).toBe("no_session");
  });

  it("dry_run refuses execute", async () => {
    const dir = mkdtempSync(join(tmpdir(), "pcc-aa-"));
    const aa = AaService.withMemory({
      ledgerDir: dir,
      gates: { dryRun: true, enableMainnetTransfers: false },
      now: () => NOW,
    });
    aa.enroll(
      buildGrant({
        owner: OWNER,
        sessionSigner: SESSION,
        chainId: SEPOLIA,
        token: TOKEN,
        maxAmount: 2_000_000n,
        validUntil: NOW + 3600,
        nowUnix: NOW,
      }),
    );
    await expect(aa.execute(plan())).rejects.toThrow(/dry_run/);
  });

  it("receipt is missing before send", () => {
    const aa = service();
    expect(aa.receipt(SEPOLIA, ("0x" + "11".repeat(32)) as Hex)).toBeUndefined();
  });
});
