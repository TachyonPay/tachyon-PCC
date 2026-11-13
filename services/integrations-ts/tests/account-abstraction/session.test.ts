/**
 * M3 session-key admission tests. No network, no bundler, no signing of live
 * keys. Covers the spec's negative cases plus persist-then-send idempotency.
 */
import { mkdtempSync, writeFileSync, chmodSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";
import { encodeFunctionData, erc20Abi, type Address, type Hex } from "viem";
import {
  KERNEL_7702_DELEGATION_ADDRESS,
  SESSION_MAX_GAS,
  SUPPORTED_7702_CHAINS,
  isMainnet7702Chain,
  isSupported7702Chain,
} from "../../src/account-abstraction/pins.js";
import { encodeErc20Transfer, APPROVE_SELECTOR } from "../../src/account-abstraction/call.js";
import { buildGrant, buildSessionPolicies, type BuildGrantInput } from "../../src/account-abstraction/grant.js";
import { loadSessionSigner, KeyRefError } from "../../src/account-abstraction/keys.js";
import { SessionLedger } from "../../src/account-abstraction/ledger.js";
import { SessionKeyExecutor, type BundlerPort } from "../../src/account-abstraction/executor.js";
import {
  assertOnboardAllowed,
  validateAuthorization,
  validateSessionCall,
  type SessionCall,
  type SessionDecision,
  type SessionGrant,
} from "../../src/account-abstraction/validate.js";

const OWNER = "0x049FdEf2550772B9F36F86757C27f5F982D0970D" as Address;
const SESSION = "0x1111111111111111111111111111111111111111" as Address;
const TOKEN = "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238" as Address; // Sepolia USDC
const OTHER_TOKEN = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48" as Address;
const RECIPIENT = "0x2222222222222222222222222222222222222222" as Address;
const SEPOLIA = SUPPORTED_7702_CHAINS.sepolia;
const NOW = 1_800_000_000;

function grant(over: Partial<BuildGrantInput> & { revoked?: boolean } = {}): SessionGrant {
  const { revoked, ...input } = over;
  const g = buildGrant({
    owner: OWNER,
    sessionSigner: SESSION,
    chainId: SEPOLIA,
    token: TOKEN,
    maxAmount: 1_000_000n,
    validUntil: NOW + 3600,
    nowUnix: NOW,
    ...input,
  });
  return revoked ? { ...g, revoked: true } : g;
}

function rejectCode(d: SessionDecision): string {
  if (d.ok) throw new Error("expected a rejection");
  return d.code;
}

function call(over: Partial<SessionCall> = {}): SessionCall {
  const amount = 500_000n;
  const to = RECIPIENT;
  return {
    chainId: SEPOLIA,
    from: OWNER,
    to: TOKEN,
    data: encodeErc20Transfer(to, amount),
    value: 0n,
    callKind: "call",
    gasLimit: 120_000n,
    approvalRef: "appr-1",
    attemptId: "attempt-1",
    ...over,
  };
}

const openGates = { dryRun: false, enableMainnetTransfers: false };

describe("session call policy (offline)", () => {
  it("accepts an in-policy Sepolia transfer under option C", () => {
    expect(validateSessionCall(call(), grant(), NOW, openGates, "execute")).toEqual({ ok: true });
  });

  it("quote-bound call refuses a substituted deposit address", () => {
    const d = validateSessionCall(
      call({ quoteDepositAddress: "0x76b4c56085ED136a8744D52bE956396624a730E8" as Address }),
      grant(),
      NOW,
    );
    expect(rejectCode(d)).toBe("quote_address_mismatch");
  });

  it("uninitialized / no session permission", () => {
    expect(rejectCode(validateSessionCall(call(), null, NOW))).toBe("no_session");
  });

  it("revoked session", () => {
    expect(rejectCode(validateSessionCall(call(), grant({ revoked: true }), NOW))).toBe("revoked");
  });

  it("expired session", () => {
    expect(rejectCode(validateSessionCall(call(), grant(), NOW + 7200))).toBe("expired");
  });

  it("wrong chainId", () => {
    expect(rejectCode(validateSessionCall(call({ chainId: 8453 }), grant(), NOW))).toBe("wrong_chain");
  });

  it("unsupported chain", () => {
    expect(rejectCode(validateSessionCall(call({ chainId: 1 }), grant(), NOW))).toBe("unsupported_chain");
  });

  it("mismatched owner / sender", () => {
    expect(rejectCode(validateSessionCall(call({ from: SESSION }), grant(), NOW))).toBe("owner_mismatch");
  });

  it("direct delegatecall", () => {
    expect(rejectCode(validateSessionCall(call({ callKind: "delegatecall" }), grant(), NOW))).toBe("delegatecall");
  });

  it("native value", () => {
    expect(rejectCode(validateSessionCall(call({ value: 1n }), grant(), NOW))).toBe("native_value");
  });

  it("unauthorized token / arbitrary target", () => {
    expect(rejectCode(validateSessionCall(call({ to: OTHER_TOKEN }), grant(), NOW))).toBe("arbitrary_target");
  });

  it("malicious token call (approve)", () => {
    const data = encodeFunctionData({
      abi: erc20Abi,
      functionName: "approve",
      args: [RECIPIENT, 1n],
    });
    expect(data.toLowerCase().startsWith(APPROVE_SELECTOR)).toBe(true);
    expect(rejectCode(validateSessionCall(call({ data }), grant(), NOW))).toBe("approve");
  });

  it("malicious selector (not transfer)", () => {
    const data = "0x23b872dd0000000000000000000000000000000000000000000000000000000000000001" as Hex; // transferFrom
    expect(rejectCode(validateSessionCall(call({ data }), grant(), NOW))).toBe("malicious_selector");
  });

  it("excessive amount", () => {
    const data = encodeErc20Transfer(RECIPIENT, 1_000_001n);
    expect(rejectCode(validateSessionCall(call({ data }), grant(), NOW))).toBe("excessive_amount");
  });

  it("option A/B recipient constraint still works when set", () => {
    const g = grant({ recipient: RECIPIENT });
    const other = encodeErc20Transfer(OWNER, 1n);
    expect(rejectCode(validateSessionCall(call({ data: other }), g, NOW))).toBe("recipient_mismatch");
    expect(validateSessionCall(call(), g, NOW)).toEqual({ ok: true });
  });

  it("excessive gas", () => {
    expect(rejectCode(validateSessionCall(call({ gasLimit: SESSION_MAX_GAS + 1n }), grant(), NOW))).toBe("excessive_gas");
  });

  it("execute still needs gates + approval", () => {
    expect(
      rejectCode(validateSessionCall(call(), grant(), NOW, { dryRun: true, enableMainnetTransfers: false }, "execute")),
    ).toBe("dry_run");
    expect(rejectCode(validateSessionCall(call({ approvalRef: "" }), grant(), NOW, openGates, "execute"))).toBe(
      "missing_approval",
    );
    expect(
      rejectCode(validateSessionCall(call({ chainId: 8453 }), grant({ chainId: 8453 }), NOW, openGates, "execute")),
    ).toBe("mainnet_disabled");
  });
});

describe("7702 authorization and onboarding", () => {
  it("invalid owner auth", () => {
    const r = validateAuthorization(
      { chainId: SEPOLIA, address: KERNEL_7702_DELEGATION_ADDRESS, nonce: 0n, signer: SESSION },
      { owner: OWNER, chainId: SEPOLIA, currentNonce: 0n },
    );
    expect(r).toMatchObject({ ok: false, code: "invalid_owner_auth" });
  });

  it("wrong chainId on authorization", () => {
    const r = validateAuthorization(
      { chainId: 8453, address: KERNEL_7702_DELEGATION_ADDRESS, nonce: 0n, signer: OWNER },
      { owner: OWNER, chainId: SEPOLIA, currentNonce: 0n },
    );
    expect(r).toMatchObject({ ok: false, code: "auth_wrong_chain" });
  });

  it("wrong implementation", () => {
    const r = validateAuthorization(
      { chainId: SEPOLIA, address: OWNER, nonce: 0n, signer: OWNER },
      { owner: OWNER, chainId: SEPOLIA, currentNonce: 0n },
    );
    expect(r).toMatchObject({ ok: false, code: "auth_wrong_implementation" });
  });

  it("replayed 7702 authorization (stale nonce)", () => {
    const r = validateAuthorization(
      { chainId: SEPOLIA, address: KERNEL_7702_DELEGATION_ADDRESS, nonce: 0n, signer: OWNER },
      { owner: OWNER, chainId: SEPOLIA, currentNonce: 1n },
    );
    expect(r).toMatchObject({ ok: false, code: "auth_replay" });
  });

  it("accepts a fresh authorization for our Kernel implementation", () => {
    const r = validateAuthorization(
      { chainId: SEPOLIA, address: KERNEL_7702_DELEGATION_ADDRESS, nonce: 3n, signer: OWNER },
      { owner: OWNER, chainId: SEPOLIA, currentNonce: 3n },
    );
    expect(r).toEqual({ ok: true });
  });

  it("wallet already delegated to an unfamiliar implementation", () => {
    const other = "0x3333333333333333333333333333333333333333";
    const r = assertOnboardAllowed(`0xef0100${other.slice(2)}`);
    expect(r).toMatchObject({ ok: false, code: "already_delegated" });
  });

  it("contract account is refused", () => {
    expect(assertOnboardAllowed("0x60806040")).toMatchObject({ ok: false, code: "not_eoa" });
  });

  it("empty code may install; our Kernel may reuse", () => {
    expect(assertOnboardAllowed("0x")).toEqual({ ok: true });
    expect(assertOnboardAllowed(`0xef0100${KERNEL_7702_DELEGATION_ADDRESS.slice(2)}`)).toEqual({ ok: true });
  });
});

describe("Base Sepolia is a 7702 testnet", () => {
  it("is supported and not mainnet-gated", () => {
    expect(isSupported7702Chain(SUPPORTED_7702_CHAINS.baseSepolia)).toBe(true);
    expect(isMainnet7702Chain(SUPPORTED_7702_CHAINS.baseSepolia)).toBe(false);
  });
});

describe("session policies never include sudo", () => {
  it("composed grant is call + timestamp + gas + rate-limit(1)", () => {
    const policies = buildSessionPolicies(grant(), NOW);
    expect(policies.map((p) => p.policyParams.type)).toEqual(["call", "timestamp", "gas", "rate-limit"]);
    expect(policies.some((p) => p.policyParams.type === "sudo")).toBe(false);
  });
});

describe("session signer key ref", () => {
  it("refuses raw hex in the ref and world-readable files", () => {
    expect(() => loadSessionSigner("11".repeat(32))).toThrow(KeyRefError);
    const dir = mkdtempSync(join(tmpdir(), "pcc-sk-"));
    const p = join(dir, "key");
    writeFileSync(p, generatePrivateKey());
    chmodSync(p, 0o644);
    expect(() => loadSessionSigner(p)).toThrow(/not 0600/);
  });

  it("loads a 0600 file and never needs the key in env", () => {
    const dir = mkdtempSync(join(tmpdir(), "pcc-sk-"));
    const p = join(dir, "key");
    const pk = generatePrivateKey();
    writeFileSync(p, pk + "\n");
    chmodSync(p, 0o600);
    const acct = loadSessionSigner(p);
    expect(acct.address).toBe(privateKeyToAccount(pk).address);
  });
});

describe("persist-then-send executor", () => {
  it("signs once; restart rebroadcasts the same hash", async () => {
    const dir = mkdtempSync(join(tmpdir(), "pcc-led-"));
    const ledger = new SessionLedger(dir);
    let signs = 0;
    let sends = 0;
    const hash = ("0x" + "ab".repeat(32)) as Hex;
    const bundler: BundlerPort = {
      simulate: async () => ({ ok: true, gasEstimate: 120000n, notes: [] }),
      sign: async () => {
        signs += 1;
        return hash;
      },
      send: async () => {
        sends += 1;
      },
    };
    const exec = new SessionKeyExecutor(openGates, ledger, bundler, () => NOW);
    const first = await exec.execute(call(), grant());
    const second = await exec.execute(call(), grant());
    expect(first).toBe(hash);
    expect(second).toBe(hash);
    expect(signs).toBe(1);
    expect(sends).toBe(2);
  });

  it("out-of-policy execute never signs", async () => {
    const dir = mkdtempSync(join(tmpdir(), "pcc-led-"));
    let signs = 0;
    const bundler: BundlerPort = {
      simulate: async () => ({ ok: true, gasEstimate: 1n, notes: [] }),
      sign: async () => {
        signs += 1;
        return ("0x" + "cd".repeat(32)) as Hex;
      },
      send: async () => {},
    };
    const exec = new SessionKeyExecutor(openGates, new SessionLedger(dir), bundler, () => NOW);
    await expect(exec.execute(call({ to: OTHER_TOKEN }), grant())).rejects.toThrow(/arbitrary_target/);
    expect(signs).toBe(0);
  });
});
