/**
 * TS mirror of `pcc_payments::ExecutionGates`. Only the exact string "false"
 * disables DRY_RUN; only the exact string "true" enables mainnet.
 */
import { isMainnet7702Chain } from "./pins.js";

export interface ExecutionGates {
  dryRun: boolean;
  enableMainnetTransfers: boolean;
}

export function gatesFromEnv(env: NodeJS.ProcessEnv = process.env): ExecutionGates {
  const dryRun = env["DRY_RUN"] === undefined || env["DRY_RUN"].trim() !== "false";
  const enableMainnetTransfers = env["ENABLE_MAINNET_TRANSFERS"]?.trim() === "true";
  return { dryRun, enableMainnetTransfers };
}

export type GateReject =
  | { ok: true }
  | { ok: false; reason: "dry_run" | "mainnet_disabled" | "missing_approval" };

export function checkBroadcast(gates: ExecutionGates, chainId: number, approvalRef: string | undefined): GateReject {
  if (gates.dryRun) return { ok: false, reason: "dry_run" };
  if (isMainnet7702Chain(chainId) && !gates.enableMainnetTransfers) {
    return { ok: false, reason: "mainnet_disabled" };
  }
  if (!approvalRef || approvalRef.trim().length === 0) return { ok: false, reason: "missing_approval" };
  return { ok: true };
}
