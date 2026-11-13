/**
 * In-process bundler for M4 mocks. Never talks to ZeroDev. Live bundler
 * wiring stays in the gated live script, not the HTTP worker.
 */
import { keccak256, toBytes, type Hex } from "viem";
import type { BundlerPort } from "./executor.js";
import type { SessionCall } from "./validate.js";

export interface MemoryReceipt {
  txHash: Hex;
  blockNumber: bigint;
  success: boolean;
  gasUsed: bigint;
}

export class MemoryBundler implements BundlerPort {
  readonly receipts = new Map<Hex, MemoryReceipt>();
  simulateOk = true;

  async simulate(_call: SessionCall): Promise<{ ok: boolean; gasEstimate: bigint; notes: string[] }> {
    if (!this.simulateOk) return { ok: false, gasEstimate: 0n, notes: ["memory bundler simulation failed"] };
    return { ok: true, gasEstimate: 120_000n, notes: ["memory bundler"] };
  }

  async sign(call: SessionCall): Promise<Hex> {
    return keccak256(toBytes(`${call.attemptId}:${call.chainId}:${call.to}:${call.data}`));
  }

  async send(userOpHash: Hex): Promise<void> {
    if (!this.receipts.has(userOpHash)) {
      this.receipts.set(userOpHash, {
        txHash: userOpHash,
        blockNumber: 1n,
        success: true,
        gasUsed: 120_000n,
      });
    }
  }

  receipt(userOpHash: Hex): MemoryReceipt | undefined {
    return this.receipts.get(userOpHash);
  }
}
