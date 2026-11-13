/**
 * Loopback EIP-1193 signer for hosted-checkout UI tests.
 *
 * Loads Wallet A from SOURCE_EXECUTOR_KEY_REF (0600 file). The browser never
 * sees the key. Signs SIWE and the Kernel enable typed data. Refuses
 * eth_sendTransaction / 7702 authorization so this process cannot broadcast.
 *
 * Bind: 127.0.0.1:1790. Origins: checkout and the Harborline shop only.
 */
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { resolve } from "node:path";
import { getAddress, type Hex, type TypedDataDefinition } from "viem";
import { loadRepoEnv } from "./_env.js";
import { base32Decode, totp } from "./_totp.js";
import { loadSessionSigner } from "../src/account-abstraction/keys.js";

loadRepoEnv();

const HOST = "127.0.0.1";
const PORT = Number(process.env["LOCAL_WALLET_PORT"] ?? "1790");
const ALLOWED = new Set([
  "http://127.0.0.1:3000",
  "http://localhost:3000",
  "http://127.0.0.1:3002",
  "http://localhost:3002",
]);
const SEND_METHODS = new Set([
  "eth_sendTransaction",
  "eth_signTransaction",
  "eth_signAuthorization",
  "wallet_signAuthorization",
  "wallet_sendCalls",
  "wallet_sendTransaction",
]);

const ownerPath = process.env["SOURCE_EXECUTOR_KEY_REF"]?.trim() || resolve(process.cwd(), "../../secrets/wallet-a.key");
const owner = loadSessionSigner(ownerPath);
const expected = (process.env["WALLET_A_ADDRESS"] ?? "").trim();
if (expected && getAddress(expected) !== getAddress(owner.address)) {
  throw new Error("local wallet file does not match WALLET_A_ADDRESS");
}

let chainId = 8453;
let lastTotpSecret: Buffer | undefined;

function cors(req: IncomingMessage, res: ServerResponse): boolean {
  const origin = req.headers.origin ?? "";
  if (origin && !ALLOWED.has(origin)) {
    res.writeHead(403, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "origin not allowed" }));
    return false;
  }
  if (origin) {
    res.setHeader("access-control-allow-origin", origin);
    res.setHeader("vary", "origin");
  }
  res.setHeader("access-control-allow-headers", "content-type");
  res.setHeader("access-control-allow-methods", "GET,POST,OPTIONS");
  return true;
}

function rpcError(id: unknown, message: string, code = -32603) {
  return { jsonrpc: "2.0", id: id ?? null, error: { code, message } };
}

function rpcOk(id: unknown, result: unknown) {
  return { jsonrpc: "2.0", id: id ?? null, result };
}

function asString(value: unknown): string {
  if (typeof value !== "string") throw new Error("expected string param");
  return value;
}

async function handleRpc(method: string, params: unknown[]): Promise<unknown> {
  if (SEND_METHODS.has(method)) {
    throw new Error("local test wallet signs only; it does not send 7702 or transfers");
  }
  switch (method) {
    case "eth_accounts":
    case "eth_requestAccounts":
      return [owner.address];
    case "eth_chainId":
      return `0x${chainId.toString(16)}`;
    case "net_version":
      return String(chainId);
    case "wallet_switchEthereumChain": {
      const hex = (params[0] as { chainId?: string } | undefined)?.chainId;
      if (hex) chainId = Number.parseInt(hex, 16);
      return null;
    }
    case "personal_sign": {
      const data = asString(params[0]);
      const from = getAddress(asString(params[1]));
      if (from !== getAddress(owner.address)) throw new Error("unknown account");
      if (/^0x[0-9a-fA-F]*$/.test(data) && data.length >= 2) {
        return owner.signMessage({ message: { raw: data as Hex } });
      }
      return owner.signMessage({ message: data });
    }
    case "eth_signTypedData_v4": {
      const from = getAddress(asString(params[0]));
      if (from !== getAddress(owner.address)) throw new Error("unknown account");
      const raw = params[1];
      const td = (typeof raw === "string" ? JSON.parse(raw) : raw) as TypedDataDefinition & {
        types: Record<string, unknown>;
      };
      const { EIP712Domain: _domainType, ...types } = td.types ?? {};
      return owner.signTypedData({
        domain: td.domain,
        types,
        primaryType: td.primaryType,
        message: td.message,
      });
    }
    default:
      throw new Error(`method not supported: ${method}`);
  }
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolveBody, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => resolveBody(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

const server = createServer(async (req, res) => {
  try {
    if (!cors(req, res)) return;
    const url = new URL(req.url ?? "/", `http://${HOST}:${PORT}`);
    if (req.method === "OPTIONS") {
      res.writeHead(204);
      res.end();
      return;
    }
    if (req.method === "GET" && url.pathname === "/health") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ ok: true, address: owner.address, signOnly: true }));
      return;
    }
    if (req.method === "GET" && url.pathname === "/totp") {
      if (!lastTotpSecret) {
        res.writeHead(404, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: "no totp yet" }));
        return;
      }
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ code: totp(lastTotpSecret) }));
      return;
    }
    if (req.method === "POST" && url.pathname === "/totp") {
      const body = JSON.parse((await readBody(req)) || "{}") as { otpauth?: string };
      const secret = new URL(body.otpauth ?? "").searchParams.get("secret") ?? "";
      if (!secret) throw new Error("missing otpauth secret");
      lastTotpSecret = base32Decode(secret);
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ code: totp(lastTotpSecret) }));
      return;
    }
    if (req.method !== "POST" || url.pathname !== "/rpc") {
      res.writeHead(404);
      res.end();
      return;
    }
    const payload = JSON.parse((await readBody(req)) || "{}") as {
      id?: unknown;
      method?: string;
      params?: unknown[];
    };
    try {
      const result = await handleRpc(payload.method ?? "", Array.isArray(payload.params) ? payload.params : []);
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(rpcOk(payload.id, result)));
    } catch (e) {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(rpcError(payload.id, e instanceof Error ? e.message : "rpc failed", -32601)));
    }
  } catch {
    res.writeHead(400, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "bad request" }));
  }
});

server.listen(PORT, HOST, () => {
  process.stderr.write(`local Wallet A signer http://${HOST}:${PORT} address=${owner.address} sign-only\n`);
});
