/**
 * Minimal HTTP server for the internal API (no framework). Every request
 * must carry `Authorization: Bearer <token>`
 * compared in constant time. Bodies are capped at 256 KiB. All errors are
 * rendered as the contract's `ErrorResponse`.
 */
import { timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { QuoteBindError, type AaService } from "../account-abstraction/service.js";
import {
  allowanceBody,
  parseAllowanceKey,
  parseDepositPlan,
  parseGrantEnable,
  parseGrantPrepare,
  parseTxHash,
  receiptBody,
  simulationBody,
} from "../account-abstraction/http.js";
import { GrantError, type SessionGrantService } from "../account-abstraction/session_grants.js";
import { WorkerError, err } from "../errors.js";
import type { NearService } from "../near/service.js";

export interface AppDeps {
  internalToken: string;
  near: NearService;
  versions: { oneClickSdk: string; zerodevSdk: string };
  aa?: AaService;
  log?: (line: string) => void;
}

const MAX_BODY = 256 * 1024;
const BASE = "/internal/v1";

export function createApp(deps: AppDeps): Server {
  const log = deps.log ?? ((l) => process.stderr.write(l + "\n"));
  return createServer(async (req, res) => {
    const started = Date.now();
    const requestId = header(req, "x-request-id") ?? "-";
    const url = new URL(req.url ?? "/", "http://internal");
    try {
      if (!authorized(req, deps.internalToken)) throw err.unauthorized();
      const result = await route(req, url, deps);
      send(res, 200, result);
    } catch (e) {
      const we = e instanceof WorkerError ? e : err.internal();
      if (!(e instanceof WorkerError)) log(`[${requestId}] internal error: ${(e as Error)?.message ?? String(e)}`);
      send(res, we.httpStatus, we.toBody());
    } finally {
      // Log only method/path/status/latency. Never bodies, never query (it
      // can contain deposit addresses that are payment-linking metadata).
      log(`[${requestId}] ${req.method} ${url.pathname} ${res.statusCode} ${Date.now() - started}ms`);
    }
  });
}

async function route(req: IncomingMessage, url: URL, deps: AppDeps): Promise<unknown> {
  const m = req.method ?? "GET";
  const p = url.pathname;

  if (m === "GET" && p === `${BASE}/health`) {
    return {
      ok: true,
      service: "pcc-integrations-ts",
      apiVersion: "1.0.0",
      oneClickSdkVersion: deps.versions.oneClickSdk,
      zerodevSdkVersion: deps.versions.zerodevSdk,
    };
  }
  if (m === "GET" && p === `${BASE}/near/assets`) {
    return deps.near.assets();
  }
  if (m === "POST" && p === `${BASE}/near/quote`) {
    return deps.near.quote(await readJson(req));
  }
  if (m === "POST" && p === `${BASE}/near/verify`) {
    const body = (await readJson(req)) as { rawResponseJson?: unknown };
    if (typeof body?.rawResponseJson !== "string") throw err.invalid("rawResponseJson (string) required");
    return deps.near.verifyRaw(body.rawResponseJson);
  }
  if (m === "GET" && p === `${BASE}/near/status`) {
    const depositAddress = url.searchParams.get("depositAddress") ?? "";
    const depositMemo = url.searchParams.get("depositMemo") ?? undefined;
    return deps.near.status(depositAddress, depositMemo);
  }
  if (m === "POST" && p === `${BASE}/near/deposit-submitted`) {
    const body = (await readJson(req)) as { depositAddress?: unknown; txHash?: unknown };
    if (typeof body?.depositAddress !== "string" || typeof body?.txHash !== "string") {
      throw err.invalid("depositAddress and txHash (strings) required");
    }
    await deps.near.depositSubmitted(body.depositAddress, body.txHash);
    return { ok: true };
  }
  if (m === "POST" && p === `${BASE}/aa/simulate`) {
    const aa = requireAa(deps);
    return simulationBody(await aa.simulate(parseDepositPlan(await readJson(req))));
  }
  if (m === "POST" && p === `${BASE}/aa/execute`) {
    const aa = requireAa(deps);
    try {
      const result = await aa.execute(parseDepositPlan(await readJson(req)));
      return { userOpHash: result.userOpHash, txHash: result.txHash };
    } catch (e) {
      throw sessionError(e);
    }
  }
  if (m === "POST" && p === `${BASE}/aa/grants/prepare`) {
    const grants = requireGrants(deps);
    try {
      return await grants.prepare(parseGrantPrepare(await readJson(req)));
    } catch (e) {
      throw sessionError(e);
    }
  }
  if (m === "POST" && p === `${BASE}/aa/grants/enable`) {
    const grants = requireGrants(deps);
    const { grantId, signature } = parseGrantEnable(await readJson(req));
    try {
      return allowanceBody(await grants.enable(grantId, signature));
    } catch (e) {
      throw sessionError(e);
    }
  }
  if (m === "POST" && p === `${BASE}/aa/grants/status`) {
    const grants = requireGrants(deps);
    const k = parseAllowanceKey(await readJson(req));
    const status = grants.status(k.chainId, k.owner, k.token, k.mandateId);
    if (!status) throw new WorkerError("INVALID_REQUEST", "no active allowance", 404);
    return allowanceBody(status);
  }
  if (m === "POST" && p === `${BASE}/aa/grants/revoke`) {
    const grants = requireGrants(deps);
    const k = parseAllowanceKey(await readJson(req));
    return { revoked: grants.revoke(k.chainId, k.owner, k.token, k.mandateId) };
  }
  if (m === "GET" && p === `${BASE}/aa/receipt`) {
    const aa = requireAa(deps);
    const chainId = Number(url.searchParams.get("chainId") ?? "");
    if (!Number.isInteger(chainId) || chainId <= 0) throw err.invalid("chainId (positive integer) required");
    const txHash = parseTxHash(url.searchParams.get("txHash"));
    const rec = aa.receipt(chainId, txHash);
    if (!rec) throw new WorkerError("INVALID_REQUEST", "receipt not found", 404);
    return receiptBody(rec);
  }
  throw new WorkerError("INVALID_REQUEST", `no route ${m} ${p}`, 404);
}

function requireAa(deps: AppDeps): AaService {
  if (!deps.aa) throw err.upstreamUnavailable("session-key executor is not configured");
  return deps.aa;
}

function requireGrants(deps: AppDeps): SessionGrantService {
  const grants = deps.aa?.grants;
  if (!grants) throw err.upstreamUnavailable("session grants are not configured");
  return grants;
}

function sessionError(e: unknown): WorkerError {
  if (e instanceof WorkerError) return e;
  if (e instanceof QuoteBindError || e instanceof GrantError) return err.sessionRejected(e.message);
  const code = (e as { code?: string } | undefined)?.code;
  const message = e instanceof Error ? e.message : String(e);
  if (typeof code === "string" && code.length > 0) return err.sessionRejected(`${code}: ${message}`);
  if (/dry_run|mainnet_disabled|missing_approval|no_session|revoked|expired|wrong_chain|unsupported_chain|owner_mismatch|delegatecall|native_value|approve|malicious_selector|arbitrary_target|excessive_amount|recipient_mismatch|excessive_gas|quote_address_mismatch|chain_not_enrolled|signer_disabled/.test(message)) {
    return err.sessionRejected(message);
  }
  return err.internal();
}

function authorized(req: IncomingMessage, token: string): boolean {
  const h = header(req, "authorization");
  if (!h || !h.startsWith("Bearer ")) return false;
  const presented = Buffer.from(h.slice("Bearer ".length));
  const expected = Buffer.from(token);
  if (presented.length !== expected.length) return false;
  return timingSafeEqual(presented, expected);
}

function header(req: IncomingMessage, name: string): string | undefined {
  const v = req.headers[name];
  return Array.isArray(v) ? v[0] : v;
}

async function readJson(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of req) {
    const b = chunk as Buffer;
    size += b.length;
    if (size > MAX_BODY) throw err.invalid("body too large");
    chunks.push(b);
  }
  const text = Buffer.concat(chunks).toString("utf8");
  if (!text) throw err.invalid("empty body");
  try {
    return JSON.parse(text);
  } catch {
    throw err.invalid("body is not valid JSON");
  }
}

function send(res: ServerResponse, status: number, body: unknown): void {
  const json = JSON.stringify(body);
  res.writeHead(status, {
    "Content-Type": "application/json",
    "Content-Length": Buffer.byteLength(json),
    "Cache-Control": "no-store",
  });
  res.end(json);
}
