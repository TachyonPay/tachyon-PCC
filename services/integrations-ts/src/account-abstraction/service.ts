/**
 * Session-key deposit service used by the internal AA routes.
 *
 * Builds ERC-20 `transfer(quoteDepositAddress, amount)` calldata from a
 * Rust `DepositPlan`. Option C still allows any recipient *on-chain*; this
 * layer binds every UserOp to the verified quote deposit address so a
 * substituted address never reaches the bundler.
 */
import { getAddress, isAddress, type Address, type Hex } from "viem";
import { MemoryBundler } from "./bundler.js";
import { encodeErc20Transfer } from "./call.js";
import { SessionKeyExecutor, type BundlerPort, type BundlerReceipt } from "./executor.js";
import type { ExecutionGates } from "./gates.js";
import { SessionLedger } from "./ledger.js";
import type { SessionGrantService } from "./session_grants.js";
import type { SessionCall, SessionDecision, SessionGrant } from "./validate.js";

export interface DepositPlanInput {
  attemptId: string;
  chainId: number;
  token: string;
  from: string;
  to: string;
  quoteDepositAddress: string;
  amount: bigint;
  maxFeeWei: bigint;
  approvalRef: string;
  gasLimit?: bigint;
  /** Autopay mandate whose allowance pays this; absent = the checkout allowance. */
  mandateId?: string;
}

export interface AaSimulation {
  ok: boolean;
  gasEstimate: bigint;
  maxFeeWei: bigint;
  senderBalance: bigint;
  notes: string[];
  rejectCode?: string;
}

export class QuoteBindError extends Error {
  readonly code = "quote_address_mismatch" as const;
  constructor(message = "quote_address_mismatch: plan.to is not the quote deposit address") {
    super(message);
    this.name = "QuoteBindError";
  }
}

function asAddress(label: string, value: string): Address {
  if (!isAddress(value)) throw new Error(`${label} is not an EVM address`);
  return getAddress(value);
}

function sessionKey(chainId: number, owner: Address): string {
  return `${chainId}:${getAddress(owner)}`;
}

export class AaService {
  private readonly sessions = new Map<string, SessionGrant>();
  /** Keys whose grant came from `grants` (vs. enrolled directly). */
  private readonly mirrored = new Set<string>();

  constructor(
    readonly executor: SessionKeyExecutor,
    private readonly bundler: BundlerPort,
    readonly grants?: SessionGrantService,
  ) {}

  static withMemory(opts: {
    ledgerDir: string;
    gates: ExecutionGates;
    bundler?: MemoryBundler;
    now?: () => number;
    grants?: SessionGrantService;
  }): AaService {
    const bundler = opts.bundler ?? new MemoryBundler();
    const executor = new SessionKeyExecutor(opts.gates, new SessionLedger(opts.ledgerDir), bundler, opts.now);
    return new AaService(executor, bundler, opts.grants);
  }

  enroll(grant: SessionGrant): void {
    this.sessions.set(sessionKey(grant.chainId, grant.owner), grant);
  }

  revoke(owner: Address, chainId: number): void {
    const key = sessionKey(chainId, owner);
    const existing = this.sessions.get(key);
    if (existing) this.sessions.set(key, { ...existing, revoked: true });
  }

  lookup(chainId: number, owner: Address): { grant: SessionGrant } | { decision: Extract<SessionDecision, { ok: false }> } {
    const exact = this.sessions.get(sessionKey(chainId, owner));
    if (exact) return { grant: exact };
    const enrolledElsewhere = [...this.sessions.values()].some((g) => getAddress(g.owner) === getAddress(owner));
    if (enrolledElsewhere) {
      return {
        decision: {
          ok: false,
          code: "chain_not_enrolled",
          reason: `owner has a session but chain ${chainId} is not enrolled`,
        },
      };
    }
    return { decision: { ok: false, code: "no_session", reason: "no session permission granted" } };
  }

  planToCall(plan: DepositPlanInput): SessionCall {
    const token = asAddress("token", plan.token);
    const from = asAddress("from", plan.from);
    const to = asAddress("to", plan.to);
    const quoteDeposit = asAddress("quoteDepositAddress", plan.quoteDepositAddress);
    if (to !== quoteDeposit) {
      throw new QuoteBindError();
    }
    if (plan.amount <= 0n) throw new Error("amount must be > 0");
    return {
      chainId: plan.chainId,
      from,
      to: token,
      data: encodeErc20Transfer(quoteDeposit, plan.amount),
      value: 0n,
      callKind: "call",
      gasLimit: plan.gasLimit ?? 120_000n,
      approvalRef: plan.approvalRef,
      attemptId: plan.attemptId,
      quoteDepositAddress: quoteDeposit,
      ...(plan.mandateId ? { mandateId: plan.mandateId } : {}),
    };
  }

  /** Mirror the payer's enabled grant into the local policy table. */
  private ensureGrant(plan: DepositPlanInput): void {
    if (!this.grants || !isAddress(plan.from) || !isAddress(plan.token)) return;
    const from = getAddress(plan.from);
    const k = sessionKey(plan.chainId, from);
    if (this.sessions.get(k)?.revoked) return;
    const active = this.grants.activeFor(plan.chainId, from, getAddress(plan.token), plan.mandateId);
    if (active) {
      this.enroll(active.grant);
      this.mirrored.add(k);
    } else if (this.mirrored.delete(k)) {
      this.sessions.delete(k);
    }
  }

  async simulate(plan: DepositPlanInput): Promise<AaSimulation> {
    this.ensureGrant(plan);
    let call: SessionCall;
    try {
      call = this.planToCall(plan);
    } catch (e) {
      const code = e instanceof QuoteBindError ? e.code : "malicious_selector";
      return {
        ok: false,
        gasEstimate: 0n,
        maxFeeWei: plan.maxFeeWei,
        senderBalance: 0n,
        notes: [e instanceof Error ? e.message : String(e)],
        rejectCode: code,
      };
    }
    const looked = this.lookup(call.chainId, call.from);
    if ("decision" in looked) {
      return {
        ok: false,
        gasEstimate: 0n,
        maxFeeWei: plan.maxFeeWei,
        senderBalance: 0n,
        notes: [looked.decision.reason],
        rejectCode: looked.decision.code,
      };
    }
    const sim = this.executor.simulate(call, looked.grant);
    if (!sim.ok) {
      return {
        ok: false,
        gasEstimate: sim.gasEstimate,
        maxFeeWei: plan.maxFeeWei,
        senderBalance: 0n,
        notes: sim.notes,
        ...(sim.rejectCode !== undefined ? { rejectCode: sim.rejectCode } : {}),
      };
    }
    let remote;
    try {
      remote = await this.bundler.simulate(call);
    } catch (e) {
      remote = { ok: false, gasEstimate: 0n, notes: [`bundler simulation error: ${e instanceof Error ? e.message : String(e)}`] };
    }
    return {
      ok: remote.ok,
      gasEstimate: remote.ok ? remote.gasEstimate : sim.gasEstimate,
      maxFeeWei: plan.maxFeeWei,
      senderBalance: remote.senderBalance ?? 0n,
      notes: ["local policy ok", ...remote.notes],
      ...(remote.ok ? {} : { rejectCode: "bundler_simulation_failed" }),
    };
  }

  async execute(plan: DepositPlanInput): Promise<{ userOpHash: Hex; txHash: Hex }> {
    this.ensureGrant(plan);
    let call: SessionCall;
    try {
      call = this.planToCall(plan);
    } catch (e) {
      throw e instanceof QuoteBindError ? e : Object.assign(new Error(e instanceof Error ? e.message : String(e)), { code: "malicious_selector" });
    }
    const looked = this.lookup(call.chainId, call.from);
    if ("decision" in looked) {
      throw Object.assign(new Error(`${looked.decision.code}: ${looked.decision.reason}`), {
        code: looked.decision.code,
      });
    }
    const hash = await this.executor.execute(call, looked.grant);
    this.grants?.consume(call.chainId, call.from, call.to, call.attemptId, call.mandateId);
    return { userOpHash: hash, txHash: this.bundler.receipt?.(hash)?.txHash ?? hash };
  }

  receipt(_chainId: number, txHash: Hex): BundlerReceipt | undefined {
    return this.bundler.receipt?.(txHash);
  }
}
