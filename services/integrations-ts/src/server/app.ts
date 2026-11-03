/**
 * Minimal HTTP server for the internal API (no framework; the surface is
 * six routes). Every request must carry `Authorization: Bearer <token>`
 * compared in constant time. Bodies are capped at 256 KiB. All errors are
 * rendered as the contract's `ErrorResponse`.
 */
import { timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { WorkerError, err } from "../errors.js";
import type { NearService } from "../near/service.js";

export interface AppDeps {
  internalToken: string;
  near: NearService;
  versions: { oneClickSdk: string; zerodevSdk: string };
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
  throw new WorkerError("INVALID_REQUEST", `no route ${m} ${p}`, 404);
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
