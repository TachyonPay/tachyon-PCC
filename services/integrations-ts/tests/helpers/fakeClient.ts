/**
 * Scriptable stand-in for `OneClickClient`. Signs echoes with a test key so
 * the official verifier accepts them unless a test deliberately tampers.
 */
import { WorkerError } from "../../src/errors.js";
import type { RawQuoteResult } from "../../src/near/client.js";
import type { OneClickClientLike } from "../../src/near/service.js";
import type { QuoteRequest, QuoteResponse, StatusResponse, TokenResponse } from "../../src/near/types.js";
import { fakeQuoteResponse, type TestSigner } from "./signer.js";

export const USDC_BASE = "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near";
export const USDC_ARB = "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near";
export const SOL_NATIVE = "nep141:sol.omft.near";

export const DEFAULT_TOKENS: TokenResponse[] = [
  { assetId: USDC_BASE, decimals: 6, blockchain: "base", symbol: "USDC", price: 1, priceUpdatedAt: "2026-11-03T00:00:00Z", contractAddress: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913" },
  { assetId: USDC_ARB, decimals: 6, blockchain: "arb", symbol: "USDC", price: 1, priceUpdatedAt: "2026-11-03T00:00:00Z", contractAddress: "0xaf88d065e77c8cc2239327c5edb3a432268e5831" },
  { assetId: SOL_NATIVE, decimals: 9, blockchain: "sol", symbol: "SOL", price: 100, priceUpdatedAt: "2026-11-03T00:00:00Z" },
];

export type QuoteHook = (req: QuoteRequest, signed: QuoteResponse) => QuoteResponse | Promise<QuoteResponse>;

export class FakeOneClickClient implements OneClickClientLike {
  hasApiKey = true;
  tokens: TokenResponse[] = DEFAULT_TOKENS;
  /** Throw this on the next postQuote call (then reset). */
  nextError: WorkerError | undefined;
  /** Mutate the signed response before returning (e.g. tamper). */
  quoteHook: QuoteHook | undefined;
  lastRequest: QuoteRequest | undefined;
  statuses = new Map<string, StatusResponse>();
  submitted: { depositAddress: string; txHash: string }[] = [];
  calls = { tokens: 0, quote: 0, status: 0 };

  constructor(public readonly signer: TestSigner) {}

  async getTokens(): Promise<TokenResponse[]> {
    this.calls.tokens += 1;
    return this.tokens;
  }

  async postQuote(req: QuoteRequest): Promise<RawQuoteResult> {
    this.calls.quote += 1;
    this.lastRequest = req;
    if (this.nextError) {
      const e = this.nextError;
      this.nextError = undefined;
      throw e;
    }
    let signed = fakeQuoteResponse(this.signer, req);
    if (this.quoteHook) signed = await this.quoteHook(req, signed);
    return { parsed: signed, rawJson: JSON.stringify(signed) };
  }

  async getStatus(depositAddress: string): Promise<StatusResponse> {
    this.calls.status += 1;
    const s = this.statuses.get(depositAddress);
    if (!s) throw new WorkerError("UPSTREAM_REJECTED", "deposit address not found", 422);
    return s;
  }

  async submitDepositTx(depositAddress: string, txHash: string): Promise<void> {
    this.submitted.push({ depositAddress, txHash });
  }
}
