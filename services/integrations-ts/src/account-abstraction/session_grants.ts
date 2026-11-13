/**
 * Per-payer spending allowances.
 *
 * `prepare` mints a fresh session key and returns the Kernel enable payload
 * (EIP-712) for up to `uses` ERC-20 transfers of one token, each capped at
 * `maxAmount`, until `validUntil`. The payer's wallet signs it once; `enable`
 * checks the signature recovers to the payer and serializes the permission
 * account. Later checkouts reuse the allowance with a TOTP code only. The
 * on-chain policies (call cap, rate limit, expiry, gas budget) are the hard
 * limit; the counters here only mirror them. Active allowances are written
 * to an optional encrypted `GrantStore`; without one, a worker restart drops
 * every allowance and payers sign again.
 */
import { randomUUID } from "node:crypto";
import { getAddress, verifyTypedData, type Address, type Hex, type TypedDataDefinition } from "viem";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";
import { buildGrant } from "./grant.js";
import { fromPersisted, type GrantStore, type PersistedGrant } from "./grant_store.js";
import { CHECKOUT_MAX_GAS_WEI } from "./pins.js";
import type { SessionGrant } from "./validate.js";

export const MAX_ALLOWANCE_USES = 20;
export const MIN_ALLOWANCE_SECONDS = 600;
export const MAX_ALLOWANCE_SECONDS = 30 * 24 * 3600;

export interface GrantSpec {
  grant: SessionGrant;
  nowUnix: number;
  sessionKey: Hex;
}

/** Chain-facing half; the ZeroDev implementation lives in kernel_grants.ts. */
export interface GrantBackend {
  /** Throws `not_delegated` when the owner lacks the Kernel 7702 delegation. */
  enableTypedData(spec: GrantSpec): Promise<TypedDataDefinition>;
  serialize(spec: GrantSpec, enableSignature: Hex): Promise<string>;
}

export interface ActiveGrant {
  grant: SessionGrant;
  mandateId?: string;
  sessionKey: Hex;
  serialized: string;
  usesRemaining: number;
  consumedAttempts: Set<string>;
}

export interface AllowanceRequest {
  owner: Address;
  chainId: number;
  token: Address;
  /** Per-transfer cap in token base units. */
  maxAmount: bigint;
  uses: number;
  validitySec: number;
  /** Autopay mandate this allowance belongs to; absent = the payer's checkout allowance. */
  mandateId?: string;
}

export interface AllowanceStatus {
  owner: Address;
  chainId: number;
  token: Address;
  maxAmount: bigint;
  usesRemaining: number;
  validUntil: number;
  mandateId?: string;
}

export interface PreparedGrantView {
  grantId: string;
  /** JSON-safe (bigints as decimal strings) for eth_signTypedData_v4. */
  typedData: unknown;
  sessionSigner: Address;
  uses: number;
  validUntil: number;
  expiresAt: string;
}

export class GrantError extends Error {
  constructor(
    readonly code: "unknown_grant" | "grant_expired" | "signature_invalid" | "not_delegated" | "invalid_allowance",
    message: string,
  ) {
    super(`${code}: ${message}`);
    this.name = "GrantError";
  }
}

interface Prepared {
  spec: GrantSpec;
  mandateId?: string;
  typedData: TypedDataDefinition;
  expiresAt: number;
}

const key = (chainId: number, owner: Address, token: Address, mandateId?: string) =>
  `${chainId}:${getAddress(owner)}:${getAddress(token)}:${mandateId ?? "checkout"}`;

export function toJsonSafe(value: unknown): unknown {
  if (typeof value === "bigint") return value.toString();
  if (Array.isArray(value)) return value.map(toJsonSafe);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, toJsonSafe(v)]));
  }
  return value;
}

export class SessionGrantService {
  private readonly prepared = new Map<string, Prepared>();
  private readonly active = new Map<string, ActiveGrant>();
  private readonly now: () => number;
  private readonly signTtlSec: number;
  private readonly store: GrantStore | undefined;

  /** Throws `GrantStoreError` when the store cannot be decrypted. */
  constructor(
    private readonly backend: GrantBackend,
    opts: { now?: () => number; signTtlSec?: number; store?: GrantStore } = {},
  ) {
    this.now = opts.now ?? (() => Math.floor(Date.now() / 1000));
    this.signTtlSec = opts.signTtlSec ?? 600;
    this.store = opts.store;
    for (const p of this.store?.load() ?? []) {
      const g = fromPersisted(p);
      this.active.set(key(g.grant.chainId, g.grant.owner, g.grant.token, g.mandateId), g);
    }
    this.dropSpent();
  }

  /** Active allowances restored from or written to the store. */
  get size(): number {
    return this.active.size;
  }

  async prepare(input: AllowanceRequest): Promise<PreparedGrantView> {
    if (input.maxAmount <= 0n) throw new GrantError("invalid_allowance", "maxAmount must be > 0");
    if (!Number.isInteger(input.uses) || input.uses < 1 || input.uses > MAX_ALLOWANCE_USES) {
      throw new GrantError("invalid_allowance", `uses must be 1..${MAX_ALLOWANCE_USES}`);
    }
    if (
      !Number.isInteger(input.validitySec) ||
      input.validitySec < MIN_ALLOWANCE_SECONDS ||
      input.validitySec > MAX_ALLOWANCE_SECONDS
    ) {
      throw new GrantError("invalid_allowance", `validitySec must be ${MIN_ALLOWANCE_SECONDS}..${MAX_ALLOWANCE_SECONDS}`);
    }
    const now = this.now();
    this.sweep(now);
    const sessionKey = generatePrivateKey();
    const sessionSigner = privateKeyToAccount(sessionKey).address;
    const grant = buildGrant({
      owner: getAddress(input.owner),
      sessionSigner,
      chainId: input.chainId,
      token: getAddress(input.token),
      maxAmount: input.maxAmount,
      validUntil: now + input.validitySec,
      nowUnix: now,
      // The on-chain gas policy is a cumulative budget across every use.
      maxGasWei: CHECKOUT_MAX_GAS_WEI * BigInt(input.uses),
      rateLimitCount: input.uses,
    });
    const spec: GrantSpec = { grant, nowUnix: now, sessionKey };
    const typedData = await this.backend.enableTypedData(spec);
    const grantId = randomUUID();
    const expiresAt = now + this.signTtlSec;
    this.prepared.set(grantId, { spec, typedData, expiresAt, ...(input.mandateId ? { mandateId: input.mandateId } : {}) });
    return {
      grantId,
      typedData: toJsonSafe(typedData),
      sessionSigner,
      uses: input.uses,
      validUntil: grant.validUntil,
      expiresAt: new Date(expiresAt * 1000).toISOString(),
    };
  }

  /** Replaces any earlier allowance for the same payer, chain, token and mandate. */
  async enable(grantId: string, signature: Hex): Promise<AllowanceStatus> {
    const p = this.prepared.get(grantId);
    if (!p) throw new GrantError("unknown_grant", "no prepared grant with that id");
    if (this.now() >= p.expiresAt) {
      this.prepared.delete(grantId);
      throw new GrantError("grant_expired", "sign window passed; prepare a new grant");
    }
    let valid = false;
    try {
      valid = await verifyTypedData({ ...p.typedData, address: p.spec.grant.owner, signature });
    } catch {
      valid = false;
    }
    if (!valid) throw new GrantError("signature_invalid", "signature does not recover to the owner");
    const serialized = await this.backend.serialize(p.spec, signature);
    this.prepared.delete(grantId);
    const { grant } = p.spec;
    this.active.set(key(grant.chainId, grant.owner, grant.token, p.mandateId), {
      grant,
      ...(p.mandateId ? { mandateId: p.mandateId } : {}),
      sessionKey: p.spec.sessionKey,
      serialized,
      usesRemaining: grant.rateLimitCount,
      consumedAttempts: new Set(),
    });
    this.persist();
    return statusOf(grant, grant.rateLimitCount, p.mandateId);
  }

  activeFor(chainId: number, owner: Address, token: Address, mandateId?: string): ActiveGrant | undefined {
    const k = key(chainId, owner, token, mandateId);
    const g = this.active.get(k);
    if (!g) return undefined;
    if (spent(g, this.now())) {
      this.active.delete(k);
      this.persist();
      return undefined;
    }
    return g;
  }

  status(chainId: number, owner: Address, token: Address, mandateId?: string): AllowanceStatus | undefined {
    const g = this.activeFor(chainId, owner, token, mandateId);
    return g ? statusOf(g.grant, g.usesRemaining, mandateId) : undefined;
  }

  /**
   * Count one executed transfer, once per payment attempt (an idempotent
   * re-execute replays the ledger instead of sending). The allowance is
   * dropped after its last use.
   */
  consume(chainId: number, owner: Address, token: Address, attemptId: string, mandateId?: string): void {
    const k = key(chainId, owner, token, mandateId);
    const g = this.active.get(k);
    if (!g || g.consumedAttempts.has(attemptId)) return;
    g.consumedAttempts.add(attemptId);
    g.usesRemaining -= 1;
    if (g.usesRemaining <= 0) this.active.delete(k);
    this.persist();
  }

  /** Destroys the session key. The on-chain permission stays installed but nothing can sign for it. */
  revoke(chainId: number, owner: Address, token: Address, mandateId?: string): boolean {
    const removed = this.active.delete(key(chainId, owner, token, mandateId));
    if (removed) this.persist();
    return removed;
  }

  private dropSpent(): void {
    const now = this.now();
    let changed = false;
    for (const [k, g] of this.active) {
      if (spent(g, now)) changed = this.active.delete(k) || changed;
    }
    if (changed) this.persist();
  }

  private persist(): void {
    this.store?.save([...this.active.values()].map(toPersisted));
  }

  private sweep(now: number): void {
    for (const [id, p] of this.prepared) if (now >= p.expiresAt) this.prepared.delete(id);
  }
}

function spent(g: ActiveGrant, now: number): boolean {
  return g.grant.revoked || g.usesRemaining <= 0 || now >= g.grant.validUntil;
}

function toPersisted(g: ActiveGrant): PersistedGrant {
  return {
    grant: {
      ...g.grant,
      maxAmount: g.grant.maxAmount.toString(),
      maxGas: g.grant.maxGas.toString(),
      maxGasWei: g.grant.maxGasWei.toString(),
    },
    ...(g.mandateId ? { mandateId: g.mandateId } : {}),
    sessionKey: g.sessionKey,
    serialized: g.serialized,
    usesRemaining: g.usesRemaining,
    consumedAttempts: [...g.consumedAttempts],
  };
}

function statusOf(grant: SessionGrant, usesRemaining: number, mandateId?: string): AllowanceStatus {
  return {
    ...(mandateId ? { mandateId } : {}),
    owner: grant.owner,
    chainId: grant.chainId,
    token: grant.token,
    maxAmount: grant.maxAmount,
    usesRemaining,
    validUntil: grant.validUntil,
  };
}
