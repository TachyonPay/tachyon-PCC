/**
 * NEAR 1Click service exposed to Rust over the internal API. It only accepts
 * the narrow `QuoteRequestDto` shape from the Rust contract, builds the
 * exact 1Click request (EXACT_OUTPUT, ORIGIN_CHAIN deposit, DESTINATION_CHAIN
 * recipient, ORIGIN_CHAIN refund, confidentiality), verifies the signature,
 * and projects a `VerifiedQuoteDto`. It never accepts a raw 1Click request
 * from a caller.
 */
import { isAddress } from "viem";
import { z } from "zod";
import { err } from "../errors.js";
import type { OneClickClient } from "./client.js";
import { Confidentiality, QuoteResponseSchema, type QuoteRequest, type QuoteResponse, type TokenResponse } from "./types.js";
import { verifyQuoteResponse } from "./verify.js";

const BaseUnits = z.string().regex(/^[0-9]+$/);

/** Sent on every quote; part of the signed payload. */
export const QUOTE_WAITING_TIME_MS = 3000;

/** Mirrors `QuoteRequestDto` in the Rust contract. */
export const QuoteRequestDtoSchema = z
  .object({
    dry: z.boolean(),
    authorizationRef: z.string().min(8).max(128),
    originAssetId: z.string().min(1),
    destinationAssetId: z.string().min(1),
    amountOut: BaseUnits,
    recipient: z.string().refine((a) => isAddress(a), "recipient must be an EVM address"),
    refundTo: z.string().refine((a) => isAddress(a), "refundTo must be an EVM address"),
    confidentiality: Confidentiality,
    slippageBps: z.number().int().min(0).max(1000),
    deadline: z.string().datetime(),
  })
  .strict();
export type QuoteRequestDto = z.infer<typeof QuoteRequestDtoSchema>;

export interface VerifiedQuoteDto {
  correlationId: string;
  signature: string;
  signatureVerified: boolean;
  dry: boolean;
  swapType: "EXACT_INPUT" | "EXACT_OUTPUT" | "FLEX_INPUT" | "ANY_INPUT";
  confidentiality: "public" | "basic" | "advanced";
  depositMode: "SIMPLE" | "MEMO";
  originAssetId: string;
  destinationAssetId: string;
  recipient: string;
  refundTo: string;
  amountIn: string;
  minAmountIn: string;
  amountOut: string;
  minAmountOut: string;
  depositAddress?: string;
  depositMemo?: string;
  quoteTimestamp: string;
  deadline?: string;
  timeEstimateSeconds: number;
  rawResponseJson: string;
}

export interface TokenEntryDto {
  assetId: string;
  blockchain: string;
  symbol: string;
  decimals: number;
  contractAddress?: string;
  priceUsd: string;
  priceUpdatedAt: string;
}

export interface SwapStatusDto {
  status: string;
  updatedAt: string;
  originTxHashes: string[];
  destinationTxHashes: string[];
  amountIn?: string;
  amountOut?: string;
  refundedAmount?: string;
}

export interface Clock {
  now(): Date;
}

export interface NearServiceOptions {
  clock?: Clock;
  tokenCacheMs?: number;
  /** Test-only override of the 1Click manager public key. Production uses the SDK default. */
  managerPublicKey?: string;
}

/** Narrow interface so tests can substitute a fake client. */
export type OneClickClientLike = Pick<OneClickClient, "hasApiKey" | "getTokens" | "postQuote" | "getStatus" | "submitDepositTx">;

export class NearService {
  /** Single-use `authorizationRef`s for executable (dry:false) quotes. */
  private readonly usedAuthRefs = new Set<string>();
  private tokenCache: { at: number; tokens: TokenResponse[] } | undefined;
  private readonly clock: Clock;
  private readonly tokenCacheMs: number;
  private readonly managerPublicKey: string | undefined;

  constructor(
    private readonly client: OneClickClientLike,
    opts: NearServiceOptions = {},
  ) {
    this.clock = opts.clock ?? { now: () => new Date() };
    this.tokenCacheMs = opts.tokenCacheMs ?? 60_000;
    this.managerPublicKey = opts.managerPublicKey;
  }

  private verifyOpts(sentRequest?: QuoteRequest) {
    const o: { sentRequest?: QuoteRequest; managerPublicKey?: string } = {};
    if (sentRequest) o.sentRequest = sentRequest;
    if (this.managerPublicKey) o.managerPublicKey = this.managerPublicKey;
    return o;
  }

  async assets(): Promise<{ tokens: TokenEntryDto[]; fetchedAt: string }> {
    const now = this.clock.now().getTime();
    if (!this.tokenCache || now - this.tokenCache.at > this.tokenCacheMs) {
      const tokens = await this.client.getTokens();
      this.tokenCache = { at: now, tokens };
    }
    return {
      fetchedAt: new Date(this.tokenCache.at).toISOString(),
      tokens: this.tokenCache.tokens.map((t) => {
        const entry: TokenEntryDto = {
          assetId: t.assetId,
          blockchain: t.blockchain,
          symbol: t.symbol,
          decimals: t.decimals,
          priceUsd: String(t.price),
          priceUpdatedAt: t.priceUpdatedAt,
        };
        if (t.contractAddress) entry.contractAddress = t.contractAddress;
        return entry;
      }),
    };
  }

  async quote(input: unknown): Promise<VerifiedQuoteDto> {
    const parsed = QuoteRequestDtoSchema.safeParse(input);
    if (!parsed.success) throw err.invalid(`quote request invalid: ${parsed.error.issues.map((i) => i.path.join(".") + " " + i.message).join("; ")}`);
    const dto = parsed.data;

    if (BigInt(dto.amountOut) === 0n) throw err.invalid("amountOut must be > 0");
    if (dto.originAssetId === dto.destinationAssetId) throw err.invalid("origin and destination asset must differ");
    const deadlineMs = Date.parse(dto.deadline);
    if (deadlineMs <= this.clock.now().getTime() + 5 * 60_000) throw err.invalid("deadline must be at least 5 minutes in the future");
    if (!this.client.hasApiKey) throw err.upstreamUnauthorized();

    // Validate assets against the live token list so we never quote an
    // asset id we have not seen 1Click advertise.
    const { tokens } = await this.assets();
    const origin = tokens.find((t) => t.assetId === dto.originAssetId);
    const destination = tokens.find((t) => t.assetId === dto.destinationAssetId);
    if (!origin) throw err.invalid(`originAssetId not in supported token list`);
    if (!destination) throw err.invalid(`destinationAssetId not in supported token list`);

    if (!dto.dry) {
      if (this.usedAuthRefs.has(dto.authorizationRef)) throw err.authRefReused(dto.authorizationRef);
      // Reserve before the network call so a concurrent duplicate is rejected.
      this.usedAuthRefs.add(dto.authorizationRef);
    }

    const request: QuoteRequest = {
      dry: dto.dry,
      depositMode: "SIMPLE",
      swapType: "EXACT_OUTPUT",
      slippageTolerance: dto.slippageBps,
      originAsset: dto.originAssetId,
      depositType: "ORIGIN_CHAIN",
      destinationAsset: dto.destinationAssetId,
      amount: dto.amountOut,
      refundTo: dto.refundTo,
      refundType: "ORIGIN_CHAIN",
      recipient: dto.recipient,
      recipientType: "DESTINATION_CHAIN",
      deadline: new Date(deadlineMs).toISOString(),
      confidentiality: dto.confidentiality,
      quoteWaitingTimeMs: QUOTE_WAITING_TIME_MS,
    };

    let result;
    try {
      result = await this.client.postQuote(request);
    } catch (e) {
      // Release the reservation only on upstream failure (no quote was issued).
      if (!dto.dry) this.usedAuthRefs.delete(dto.authorizationRef);
      throw e;
    }

    verifyQuoteResponse(result.parsed, this.verifyOpts(request));
    return project(result.parsed, result.rawJson, true);
  }

  /** Re-verify a previously stored raw response (e.g. after a Rust restart). */
  verifyRaw(rawResponseJson: string): VerifiedQuoteDto {
    let json: unknown;
    try {
      json = JSON.parse(rawResponseJson);
    } catch {
      throw err.invalid("rawResponseJson is not JSON");
    }
    const parsed = QuoteResponseSchema.safeParse(json);
    if (!parsed.success) throw err.invalid("rawResponseJson does not match the 1Click quote schema");
    verifyQuoteResponse(parsed.data, this.verifyOpts());
    return project(parsed.data, rawResponseJson, true);
  }

  async status(depositAddress: string, depositMemo?: string): Promise<SwapStatusDto> {
    if (!depositAddress) throw err.invalid("depositAddress required");
    if (!this.client.hasApiKey) throw err.upstreamUnauthorized();
    const s = await this.client.getStatus(depositAddress, depositMemo);
    // /v0/status echoes the signed quote but drops `quoteWaitingTimeMs` from
    // quoteRequest; the signature covers it, so restore the value we always send.
    const qr = s.quoteResponse;
    verifyQuoteResponse(
      {
        ...qr,
        correlationId: qr.correlationId ?? s.correlationId,
        quoteRequest: { ...qr.quoteRequest, quoteWaitingTimeMs: qr.quoteRequest.quoteWaitingTimeMs ?? QUOTE_WAITING_TIME_MS },
      },
      this.verifyOpts(),
    );
    if (qr.quote.depositAddress?.toLowerCase() !== depositAddress.toLowerCase()) {
      throw err.signatureInvalid("status quote is for a different deposit address");
    }
    const out: SwapStatusDto = {
      status: s.status,
      updatedAt: s.updatedAt,
      originTxHashes: s.swapDetails.originChainTxHashes.map((t) => t.hash),
      destinationTxHashes: s.swapDetails.destinationChainTxHashes.map((t) => t.hash),
    };
    if (s.swapDetails.amountIn) out.amountIn = s.swapDetails.amountIn;
    if (s.swapDetails.amountOut) out.amountOut = s.swapDetails.amountOut;
    if (s.swapDetails.refundedAmount) out.refundedAmount = s.swapDetails.refundedAmount;
    return out;
  }

  async depositSubmitted(depositAddress: string, txHash: string): Promise<void> {
    if (!depositAddress || !/^0x[0-9a-fA-F]{64}$/.test(txHash)) throw err.invalid("depositAddress and 0x-prefixed 32-byte txHash required");
    if (!this.client.hasApiKey) throw err.upstreamUnauthorized();
    await this.client.submitDepositTx(depositAddress, txHash);
  }
}

export function project(r: QuoteResponse, rawJson: string, signatureVerified: boolean): VerifiedQuoteDto {
  const q = r.quote;
  const req = r.quoteRequest;
  const dto: VerifiedQuoteDto = {
    correlationId: r.correlationId,
    signature: r.signature,
    signatureVerified,
    dry: req.dry,
    swapType: req.swapType,
    confidentiality: req.confidentiality ?? "public",
    depositMode: req.depositMode ?? "SIMPLE",
    originAssetId: req.originAsset,
    destinationAssetId: req.destinationAsset,
    recipient: req.recipient,
    refundTo: req.refundTo,
    amountIn: q.amountIn,
    minAmountIn: q.minAmountIn,
    amountOut: q.amountOut,
    minAmountOut: q.minAmountOut,
    quoteTimestamp: r.timestamp,
    timeEstimateSeconds: Math.round(q.timeEstimate),
    rawResponseJson: rawJson,
  };
  if (q.depositAddress) dto.depositAddress = q.depositAddress;
  if (q.depositMemo) dto.depositMemo = q.depositMemo;
  if (q.deadline) dto.deadline = q.deadline;
  return dto;
}
