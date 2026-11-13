/**
 * Persist-then-send ledger for session-key UserOps. One record per attempt id;
 * a restart rebroadcasts the same userOpHash instead of constructing a second op.
 */
import { mkdirSync, readFileSync, renameSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import type { Hex } from "viem";

export interface UserOpRecord {
  attemptId: string;
  chainId: number;
  userOpHash: Hex;
  approvalRef: string;
}

export class SessionLedger {
  constructor(private readonly dir: string) {
    mkdirSync(dir, { recursive: true });
  }

  private path(id: string): string {
    return join(this.dir, `${id}.json`);
  }

  get(attemptId: string): UserOpRecord | undefined {
    const p = this.path(attemptId);
    if (!existsSync(p)) return undefined;
    const rec = JSON.parse(readFileSync(p, "utf8")) as UserOpRecord;
    return rec;
  }

  put(rec: UserOpRecord): void {
    const existing = this.get(rec.attemptId);
    if (existing) {
      if (existing.userOpHash !== rec.userOpHash) {
        throw new Error("ledger record hash mismatch; refusing to overwrite");
      }
      return;
    }
    const tmp = `${this.path(rec.attemptId)}.tmp`;
    writeFileSync(tmp, JSON.stringify(rec, null, 2));
    renameSync(tmp, this.path(rec.attemptId));
  }
}
