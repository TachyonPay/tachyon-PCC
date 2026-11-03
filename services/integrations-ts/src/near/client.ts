/**
 * Typed HTTP client for the live 1Click REST API.
 *
 * Why typed fetch instead of `OneClickService` from the SDK: the SDK client
 * is regenerated from the same OpenAPI and would work (it supports
 * `OpenAPI.HEADERS` for `X-API-Key`), but it throws generic `ApiError`s,
 * hides the raw response bytes we must store immutably, and buffers through
 * axios. A 150-line fetch wrapper gives us: exact raw JSON retention,
 * explicit timeout handling, zod validation, and stable error codes. We
 * still use the SDK for the one thing that matters most: the official
 * `verifyQuoteSignature()` implementation (see verify.ts).
 */
import { err } from "../errors.js";
import {
  BadRequestSchema,
  QuoteResponseSchema,
  StatusResponseSchema,
  TokensResponseSchema,
  type QuoteRequest,
  type QuoteResponse,
  type StatusResponse,
  type TokenResponse,
} from "./types.js";

export type FetchLike = (input: string, init: RequestInit) => Promise<Response>;

export interface OneClickClientOptions {
  baseUrl: string;
  /** Sent as `X-API-Key` (recommended scheme per live OpenAPI; JWT is legacy). */
  apiKey: string | undefined;
  timeoutMs: number;
  fetchImpl?: FetchLike;
}

export interface RawQuoteResult {
  parsed: QuoteResponse;
  /** Exact response body as received. Stored immutably by Rust. */
  rawJson: string;
}

export class OneClickClient {
  private readonly fetchImpl: FetchLike;

  constructor(private readonly opts: OneClickClientOptions) {
    this.fetchImpl = opts.fetchImpl ?? ((input, init) => fetch(input, init));
  }

  get hasApiKey(): boolean {
    return this.opts.apiKey !== undefined && this.opts.apiKey.length > 0;
  }

  async getTokens(): Promise<TokenResponse[]> {
    const text = await this.request("GET", "/v0/tokens");
    return parseWith(TokensResponseSchema, text, "GET /v0/tokens");
  }

  async postQuote(req: QuoteRequest): Promise<RawQuoteResult> {
    const text = await this.request("POST", "/v0/quote", req);
    const parsed = parseWith(QuoteResponseSchema, text, "POST /v0/quote");
    return { parsed, rawJson: text };
  }

  async getStatus(depositAddress: string, depositMemo?: string): Promise<StatusResponse> {
    const q = new URLSearchParams({ depositAddress });
    if (depositMemo) q.set("depositMemo", depositMemo);
    const text = await this.request("GET", `/v0/status?${q.toString()}`);
    return parseWith(StatusResponseSchema, text, "GET /v0/status");
  }

  async submitDepositTx(depositAddress: string, txHash: string, depositMemo?: string): Promise<void> {
    const body: Record<string, string> = { txHash, depositAddress };
    if (depositMemo) body["depositMemo"] = depositMemo;
    await this.request("POST", "/v0/deposit/submit", body);
  }

  private async request(method: "GET" | "POST", path: string, body?: unknown): Promise<string> {
    const headers: Record<string, string> = { Accept: "application/json" };
    if (body !== undefined) headers["Content-Type"] = "application/json";
    if (this.opts.apiKey) headers["X-API-Key"] = this.opts.apiKey;

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.opts.timeoutMs);
    const init: RequestInit = { method, headers, signal: controller.signal, redirect: "error" };
    if (body !== undefined) init.body = JSON.stringify(body);
    let res: Response;
    try {
      res = await this.fetchImpl(`${this.opts.baseUrl}${path}`, init);
    } catch (e) {
      if (isAbort(e)) throw err.upstreamTimeout();
      throw err.upstreamUnavailable(`network error calling 1Click ${method} ${pathOnly(path)}`);
    } finally {
      clearTimeout(timer);
    }

    const text = await res.text().catch(() => "");
    if (res.ok) return text;

    if (res.status === 401 || res.status === 403) throw err.upstreamUnauthorized();
    if (res.status === 429) throw err.upstreamRateLimited();
    if (res.status === 404 && path.startsWith("/v0/status")) throw err.upstreamRejected("deposit address not found");
    if (res.status === 400) {
      const msg = safeMessage(text);
      // 1Click signals "no quotes" and liquidity problems as 400s with a message.
      if (/no quote|no route|liquidity|not supported|unsupported/i.test(msg)) throw err.noRoute(msg);
      throw err.upstreamRejected(msg);
    }
    if (res.status >= 500) throw err.upstreamUnavailable(`1Click returned ${res.status}`);
    throw err.upstreamRejected(`1Click returned ${res.status}: ${safeMessage(text)}`);
  }
}

function parseWith<T>(schema: { safeParse: (v: unknown) => { success: true; data: T } | { success: false; error: unknown } }, text: string, ctx: string): T {
  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch {
    throw err.malformedUpstream(`${ctx}: response is not JSON`);
  }
  const r = schema.safeParse(json);
  if (!r.success) throw err.malformedUpstream(`${ctx}: response failed schema validation`, r.error);
  return r.data;
}

function safeMessage(text: string): string {
  try {
    const r = BadRequestSchema.safeParse(JSON.parse(text));
    if (r.success) return r.data.message.slice(0, 300);
  } catch {
    /* fallthrough */
  }
  return text.slice(0, 300) || "bad request";
}

function isAbort(e: unknown): boolean {
  return typeof e === "object" && e !== null && (e as { name?: string }).name === "AbortError";
}

function pathOnly(p: string): string {
  return p.split("?")[0] ?? p;
}
