/**
 * Runtime schemas for the live 1Click API (OpenAPI v0.1.10, fetched 2026-11-03
 * from https://1click.chaindefuser.com/docs/v0/openapi.yaml).
 *
 * We validate every upstream payload with zod before touching it. The SDK's
 * generated TypeScript types are compile-time only; these schemas are the
 * runtime guard against drift, truncation and injection.
 */
import { z } from "zod";

/** Decimal integer string. Amounts are never numbers. */
export const BaseUnitsString = z.string().regex(/^[0-9]+$/, "amount must be a base-unit integer string");

export const SwapType = z.enum(["EXACT_INPUT", "EXACT_OUTPUT", "FLEX_INPUT", "ANY_INPUT"]);
export const DepositType = z.enum(["ORIGIN_CHAIN", "INTENTS", "CONFIDENTIAL_INTENTS"]);
export const RefundType = z.enum(["ORIGIN_CHAIN", "INTENTS", "CONFIDENTIAL_INTENTS"]);
export const RecipientType = z.enum(["DESTINATION_CHAIN", "INTENTS", "CONFIDENTIAL_INTENTS"]);
export const DepositMode = z.enum(["SIMPLE", "MEMO"]);
export const Confidentiality = z.enum(["public", "basic", "advanced"]);

export const OneClickStatus = z.enum([
  "KNOWN_DEPOSIT_TX",
  "PENDING_DEPOSIT",
  "INCOMPLETE_DEPOSIT",
  "PROCESSING",
  "SUCCESS",
  "REFUNDED",
  "FAILED",
]);
export type OneClickStatus = z.infer<typeof OneClickStatus>;

export const QuoteRequestSchema = z
  .object({
    dry: z.boolean(),
    depositMode: DepositMode.optional(),
    swapType: SwapType,
    slippageTolerance: z.number().int().nonnegative(),
    originAsset: z.string().min(1),
    depositType: DepositType,
    destinationAsset: z.string().min(1),
    amount: BaseUnitsString,
    refundTo: z.string().min(1),
    refundType: RefundType,
    recipient: z.string().min(1),
    recipientType: RecipientType,
    deadline: z.string().datetime(),
    confidentiality: Confidentiality.optional(),
    referral: z.string().optional(),
    quoteWaitingTimeMs: z.number().int().nonnegative().optional(),
    sessionId: z.string().optional(),
    connectedWallets: z.array(z.string()).optional(),
  })
  .loose();
export type QuoteRequest = z.infer<typeof QuoteRequestSchema>;

export const QuoteSchema = z
  .object({
    depositAddress: z.string().optional(),
    depositMemo: z.string().optional(),
    amountIn: BaseUnitsString,
    amountInFormatted: z.string(),
    amountInUsd: z.string(),
    minAmountIn: BaseUnitsString,
    amountOut: BaseUnitsString,
    amountOutFormatted: z.string(),
    amountOutUsd: z.string(),
    minAmountOut: BaseUnitsString,
    deadline: z.string().datetime().optional(),
    timeWhenInactive: z.string().datetime().optional(),
    timeEstimate: z.number().nonnegative(),
    refundFee: BaseUnitsString.optional(),
    withdrawFee: BaseUnitsString.optional(),
  })
  .loose();

/** Echoed request as returned by 1Click (may contain nulls for optionals). */
export const EchoedQuoteRequestSchema = QuoteRequestSchema.extend({
  confidentiality: Confidentiality.nullable().optional(),
  referral: z.string().nullable().optional(),
  depositMode: DepositMode.nullable().optional(),
  quoteWaitingTimeMs: z.number().nullable().optional(),
});

export const QuoteResponseSchema = z
  .object({
    correlationId: z.string().min(1),
    timestamp: z.string().datetime(),
    signature: z.string().min(1),
    quoteRequest: EchoedQuoteRequestSchema,
    quote: QuoteSchema,
  })
  .loose();
export type QuoteResponse = z.infer<typeof QuoteResponseSchema>;

export const TokenResponseSchema = z
  .object({
    assetId: z.string().min(1),
    decimals: z.number().int().min(0).max(255),
    blockchain: z.string().min(1),
    symbol: z.string(),
    price: z.union([z.number(), z.string()]),
    priceUpdatedAt: z.string(),
    contractAddress: z.string().optional(),
    coingeckoId: z.string().optional(),
  })
  .loose();
export type TokenResponse = z.infer<typeof TokenResponseSchema>;
export const TokensResponseSchema = z.array(TokenResponseSchema);

const TransactionDetails = z
  .object({
    hash: z.string(),
    explorerUrl: z.string().optional(),
  })
  .loose();

/** 1Click /v0/status sometimes returns amounts as JSON numbers, not strings. */
const StatusAmount = z.union([
  BaseUnitsString,
  z
    .number()
    .int()
    .nonnegative()
    .transform((n) => String(n)),
]);

export const SwapDetailsSchema = z
  .object({
    originChainTxHashes: z.array(TransactionDetails).optional().default([]),
    destinationChainTxHashes: z.array(TransactionDetails).optional().default([]),
    amountIn: StatusAmount.nullish(),
    amountOut: StatusAmount.nullish(),
    refundedAmount: StatusAmount.nullish(),
  })
  .loose();

/** Nested quote on /v0/status omits correlationId (it lives on the status body). */
const StatusEmbeddedQuoteSchema = QuoteResponseSchema.extend({
  correlationId: z.string().min(1).optional(),
});

export const StatusResponseSchema = z
  .object({
    correlationId: z.string(),
    quoteResponse: StatusEmbeddedQuoteSchema,
    status: OneClickStatus,
    updatedAt: z.string().datetime(),
    swapDetails: SwapDetailsSchema,
  })
  .loose();
export type StatusResponse = z.infer<typeof StatusResponseSchema>;

export const BadRequestSchema = z.object({ message: z.string() }).loose();
