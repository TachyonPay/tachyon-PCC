/**
 * Session-key deposit executor. Bundler I/O is injected so unit tests never
 * hit ZeroDev. Live wiring (create7702KernelAccountClient) belongs in the
 * Sepolia script after the owner approves that exact UserOp.
 *
 * Sign → persist userOpHash → send. A restart with the same attempt id
 * rebroadcasts the ledgered hash and never constructs a second op.
 */
import type { Hex } from "viem";
import { checkBroadcast, type ExecutionGates } from "./gates.js";
import type { SessionLedger } from "./ledger.js";
import { validateSessionCall, type SessionCall, type SessionGrant } from "./validate.js";

export interface BundlerReceipt {
  txHash: Hex;
  blockNumber: bigint;
  success: boolean;
  gasUsed: bigint;
}

export interface BundlerSimulation {
  ok: boolean;
  gasEstimate: bigint;
  notes: string[];
  senderBalance?: bigint;
}

export interface BundlerPort {
  /** Must not broadcast. */
  simulate(call: SessionCall): Promise<BundlerSimulation>;
  /** Sign the UserOp and return its hash. Must not broadcast. */
  sign(call: SessionCall): Promise<Hex>;
  /** Broadcast a previously signed UserOp. Idempotent for an already-known hash. */
  send(userOpHash: Hex): Promise<void>;
  receipt?(userOpHash: Hex): BundlerReceipt | undefined;
}

export interface SessionSimulation {
  ok: boolean;
  gasEstimate: bigint;
  notes: string[];
  rejectCode?: string;
}

export class SessionKeyExecutor {
  constructor(
    private readonly gates: ExecutionGates,
    private readonly ledger: SessionLedger,
    private readonly bundler: BundlerPort,
    private readonly now: () => number = () => Math.floor(Date.now() / 1000),
  ) {}

  simulate(call: SessionCall, grant: SessionGrant | null): SessionSimulation {
    const v = validateSessionCall(call, grant, this.now(), this.gates, "simulate");
    if (!v.ok) {
      return { ok: false, gasEstimate: 0n, notes: [v.reason], rejectCode: v.code };
    }
    return { ok: true, gasEstimate: call.gasLimit, notes: ["local policy ok"] };
  }

  async execute(call: SessionCall, grant: SessionGrant | null): Promise<Hex> {
    const gate = checkBroadcast(this.gates, call.chainId, call.approvalRef);
    if (!gate.ok) throw new Error(gate.reason);
    const v = validateSessionCall(call, grant, this.now(), this.gates, "execute");
    if (!v.ok) throw new Error(`${v.code}: ${v.reason}`);

    const existing = this.ledger.get(call.attemptId);
    if (existing) {
      await this.bundler.send(existing.userOpHash);
      return existing.userOpHash;
    }

    const sim = this.simulate(call, grant);
    if (!sim.ok) throw new Error(`preflight failed; refusing to sign: ${sim.notes.join("; ")}`);

    const remote = await this.bundler.simulate(call);
    if (!remote.ok) throw new Error(`bundler simulation failed: ${remote.notes.join("; ")}`);

    const hash = await this.bundler.sign(call);
    this.ledger.put({
      attemptId: call.attemptId,
      chainId: call.chainId,
      userOpHash: hash,
      approvalRef: call.approvalRef,
    });
    await this.bundler.send(hash);
    return hash;
  }
}
