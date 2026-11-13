import { getAddress, isAddress, isHex, type Address, type Hex } from "viem";
import { err } from "../errors.js";
import type { AaSimulation, DepositPlanInput } from "./service.js";
import type { BundlerReceipt } from "./executor.js";
import type { AllowanceRequest, AllowanceStatus } from "./session_grants.js";

function reqString(body: Record<string, unknown>, key: string): string {
  const v = body[key];
  if (typeof v !== "string" || v.trim().length === 0) throw err.invalid(`${key} (string) required`);
  return v;
}

function parseBigInt(label: string, raw: string): bigint {
  if (!/^[0-9]+$/.test(raw)) throw err.invalid(`${label} must be a decimal integer string`);
  return BigInt(raw);
}

function positiveInt(b: Record<string, unknown>, key: string): number {
  const v = b[key];
  if (typeof v !== "number" || !Number.isInteger(v) || v <= 0) throw err.invalid(`${key} (positive integer) required`);
  return v;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function optionalMandate(b: Record<string, unknown>): { mandateId?: string } {
  const v = b.mandateId;
  if (v === undefined || v === null) return {};
  if (typeof v !== "string" || !UUID.test(v)) throw err.invalid("mandateId must be a UUID");
  return { mandateId: v.toLowerCase() };
}

/** Owner, chain, token and (for autopay) mandate identify one allowance. */
export function parseAllowanceKey(body: unknown): { owner: Address; chainId: number; token: Address; mandateId?: string } {
  if (!body || typeof body !== "object") throw err.invalid("body required");
  const b = body as Record<string, unknown>;
  const chainId = positiveInt(b, "chainId");
  const owner = reqString(b, "owner");
  const token = reqString(b, "token");
  if (!isAddress(owner) || !isAddress(token)) throw err.invalid("owner and token must be EVM addresses");
  return { owner: getAddress(owner), chainId, token: getAddress(token), ...optionalMandate(b) };
}

export function parseGrantPrepare(body: unknown): AllowanceRequest {
  const k = parseAllowanceKey(body);
  const b = body as Record<string, unknown>;
  const maxAmount = parseBigInt("maxAmount", reqString(b, "maxAmount"));
  if (maxAmount <= 0n) throw err.invalid("maxAmount must be > 0");
  return { ...k, maxAmount, uses: positiveInt(b, "uses"), validitySec: positiveInt(b, "validitySec") };
}

export function parseGrantEnable(body: unknown): { grantId: string; signature: Hex } {
  if (!body || typeof body !== "object") throw err.invalid("body required");
  const b = body as Record<string, unknown>;
  const signature = reqString(b, "signature");
  if (!isHex(signature)) throw err.invalid("signature must be 0x-hex");
  return { grantId: reqString(b, "grantId"), signature };
}

export function allowanceBody(s: AllowanceStatus) {
  return {
    owner: s.owner,
    chainId: s.chainId,
    token: s.token,
    maxAmount: s.maxAmount.toString(),
    usesRemaining: s.usesRemaining,
    validUntil: s.validUntil,
    ...(s.mandateId ? { mandateId: s.mandateId } : {}),
  };
}

export function parseDepositPlan(body: unknown): DepositPlanInput {
  if (!body || typeof body !== "object") throw err.invalid("body required");
  const b = body as Record<string, unknown>;
  const chainId = b.chainId;
  if (typeof chainId !== "number" || !Number.isInteger(chainId) || chainId <= 0) {
    throw err.invalid("chainId (positive integer) required");
  }
  const plan: DepositPlanInput = {
    attemptId: reqString(b, "attemptId"),
    chainId,
    token: reqString(b, "token"),
    from: reqString(b, "from"),
    to: reqString(b, "to"),
    quoteDepositAddress: reqString(b, "quoteDepositAddress"),
    amount: parseBigInt("amount", reqString(b, "amount")),
    maxFeeWei: parseBigInt("maxFeeWei", reqString(b, "maxFeeWei")),
    approvalRef: reqString(b, "approvalRef"),
    ...optionalMandate(b),
  };
  if (b.gasLimit !== undefined) {
    if (typeof b.gasLimit !== "string") throw err.invalid("gasLimit must be a decimal integer string");
    plan.gasLimit = parseBigInt("gasLimit", b.gasLimit);
  }
  return plan;
}

export function simulationBody(sim: AaSimulation): Record<string, unknown> {
  const body: Record<string, unknown> = {
    ok: sim.ok,
    gasEstimate: Number(sim.gasEstimate),
    maxFeeWei: sim.maxFeeWei.toString(),
    senderBalance: sim.senderBalance.toString(),
    notes: sim.notes,
  };
  if (sim.rejectCode !== undefined) body.rejectCode = sim.rejectCode;
  return body;
}

export function receiptBody(rec: BundlerReceipt): Record<string, unknown> {
  return {
    txHash: rec.txHash,
    blockNumber: Number(rec.blockNumber),
    success: rec.success,
    gasUsed: Number(rec.gasUsed),
  };
}

export function parseTxHash(raw: string | null): Hex {
  if (!raw || !isHex(raw) || raw.length !== 66) throw err.invalid("txHash must be a 0x-prefixed 32-byte hash");
  return raw;
}
