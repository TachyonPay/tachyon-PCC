#!/usr/bin/env node
/**
 * Harborline merchant backend. The API key never leaves this process.
 * Browser only talks to this server; this server talks to pcc-api.
 */
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { cameraJpeg, enqueueIfPaid, printerView, startWorker, stopPrint } from "./bambu.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const publicDir = path.join(here, "public");

// pay.tachyon.pe proxies /v1 to the API. Use it from this laptop: api.tachyon.pe
// often fails local getaddrinfo even when dig works.
const API = process.env.API_BASE_URL ?? "https://pay.tachyon.pe";
const HOST = process.env.SHOP_HOST ?? "127.0.0.1";
const PORT = Number(process.env.SHOP_PORT ?? 3002);
const AMOUNT = process.env.AMOUNT_OUT ?? "200000"; // 0.20 USDC, 6 decimals
const SHOP_ORIGIN = process.env.SHOP_ORIGIN ?? `http://${HOST}:${PORT}`;
const SUCCESS_URL = process.env.SUCCESS_URL ?? `${SHOP_ORIGIN}/return.html`;
const RECEIVING = process.env.WALLET_B_ADDRESS ?? "0x1111111111111111111111111111111111111111";
const PAYOUT = {
  receiving_address: RECEIVING,
  chain_id: 42161,
  decimals: 6,
  one_click_asset_id: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near",
  token_address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831",
};

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".svg": "image/svg+xml",
  ".ico": "image/svg+xml",
};

const DASHBOARD_ORIGINS = new Set([
  "http://127.0.0.1:3001",
  "http://localhost:3001",
  "https://dash.tachyon.pe",
]);

const dataDir = path.join(here, ".data");
const keyFile = path.join(dataDir, "merchant.key");
const ordersFile = path.join(dataDir, "orders.json");

/** Public hashes from the live Mini Hull settlement. No secrets. */
const LIVE_SETTLEMENT = {
  id: "edf8e02f-5d16-44d1-8019-44f1a4b96f5c",
  merchant_id: "harborline",
  merchant_order_id: "boat-1794424198618",
  product: "Mini Hull",
  status: "succeeded",
  output_amount: "200000",
  quoted_input: "207419",
  pay_chain: "Base",
  settle_chain: "Arbitrum",
  pay_asset: "USDC",
  settle_asset: "USDC",
  payer: "0x049FdEf2550772B9F36F86757C27f5F982D0970D",
  deposit_address: "0x09e6cebc839Eeaa2D835c3F4EC08f71E58eC1B4D",
  receiving_address: "0x049FdEf2550772B9F36F86757C27f5F982D0970D",
  arb_from: "0x2cff890f0378a11913b6129b2e97417a2c302680",
  base_tx: "0xadc17feb4398a369eef528ac07087bc86e270e659ae26d645f2e0b3ef1ce16f1",
  arb_tx: "0x243bb335c28357bdbb67c1b268a0e1ca0fbb75fdbd5ae489c6825ea7b1ccf0e1",
  user_op: "0xfa0d93b7adbe15da69da916b99889965300338e9a66b4aae8eef22d76ff3027f",
  checkout_session_id: "edf8e02f-5d16-44d1-8019-44f1a4b96f5c",
  attempt_id: "f9829dd5-fa8b-4926-b04a-0cb6fb0ba476",
  checkout_url: "",
  expires_at: "2026-11-12T00:00:00.000Z",
  paid_at: "2026-11-11T19:11:19.000Z",
};

let merchantKey = process.env.MERCHANT_API_KEY?.trim() ?? "";
let merchantName = "Harborline";
/** @type {Map<string, object>} */
const orders = new Map();

function loadSavedKey() {
  try {
    return fs.readFileSync(keyFile, "utf8").trim();
  } catch {
    return "";
  }
}

function saveKey(key) {
  fs.mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  fs.writeFileSync(keyFile, key, { mode: 0o600 });
}

function loadOrders() {
  try {
    const parsed = JSON.parse(fs.readFileSync(ordersFile, "utf8"));
    const list = Array.isArray(parsed) ? parsed : parsed.orders;
    if (!Array.isArray(list)) return;
    for (const row of list) {
      if (row?.merchant_order_id || row?.id) upsertOrder(row, false);
    }
  } catch {
    // first boot
  }
}

function persistOrders() {
  fs.mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  const list = [...orders.values()].sort((a, b) => String(b.paid_at ?? "").localeCompare(String(a.paid_at ?? "")));
  fs.writeFileSync(ordersFile, `${JSON.stringify(list, null, 2)}\n`, { mode: 0o600 });
}

function orderKey(row) {
  return row.merchant_order_id || row.id;
}

function upsertOrder(patch, write = true) {
  const key = orderKey(patch);
  if (!key) return null;
  const prev = [...orders.values()].find((o) => o.merchant_order_id === patch.merchant_order_id || o.id === patch.id) ?? {};
  const next = {
    product: "Mini Hull",
    pay_chain: "Base",
    settle_chain: "Arbitrum",
    pay_asset: "USDC",
    settle_asset: "USDC",
    receiving_address: PAYOUT.receiving_address,
    ...prev,
    ...patch,
    merchant_id: patch.merchant_id ?? prev.merchant_id ?? "harborline",
  };
  orders.delete(orderKey(prev));
  orders.set(orderKey(next), next);
  if (write) persistOrders();
  return next;
}

function findOrder({ intent, order, session }) {
  for (const row of orders.values()) {
    if (intent && (row.id === intent || row.checkout_session_id === intent)) return row;
    if (order && row.merchant_order_id === order) return row;
    if (session && row.checkout_session_id === session) return row;
  }
  return null;
}

function publicOrder(row) {
  return {
    id: row.id,
    merchant_id: row.merchant_id ?? "harborline",
    merchant_order_id: row.merchant_order_id,
    product: row.product ?? "Mini Hull",
    status: row.status,
    output_amount: row.output_amount,
    quoted_input: row.quoted_input ?? null,
    pay_chain: row.pay_chain ?? "Base",
    settle_chain: row.settle_chain ?? "Arbitrum",
    pay_asset: row.pay_asset ?? "USDC",
    settle_asset: row.settle_asset ?? "USDC",
    payer: row.payer ?? null,
    deposit_address: row.deposit_address ?? null,
    receiving_address: row.receiving_address ?? PAYOUT.receiving_address,
    arb_from: row.arb_from ?? null,
    base_tx: row.base_tx ?? null,
    arb_tx: row.arb_tx ?? null,
    user_op: row.user_op ?? null,
    checkout_session_id: row.checkout_session_id ?? "",
    checkout_url: row.checkout_url ?? "",
    expires_at: row.expires_at ?? new Date(Date.now() + 3600_000).toISOString(),
    attempt_id: row.attempt_id ?? null,
    paid_at: row.paid_at ?? null,
    print: row.print
      ? {
          status: row.print.status,
          error: row.print.error ?? null,
          queued_at: row.print.queued_at ?? null,
          started_at: row.print.started_at ?? null,
        }
      : null,
    explorers: {
      base: row.base_tx ? `https://basescan.org/tx/${row.base_tx}` : null,
      arbitrum: row.arb_tx ? `https://arbiscan.io/tx/${row.arb_tx}` : null,
      confidential: `${SHOP_ORIGIN}/confidential.html?order=${encodeURIComponent(row.merchant_order_id)}`,
    },
  };
}

function corsHeaders(req) {
  const origin = req.headers.origin ?? "";
  if (!DASHBOARD_ORIGINS.has(origin)) return {};
  return {
    "access-control-allow-origin": origin,
    "access-control-allow-methods": "GET, OPTIONS",
    "access-control-allow-headers": "accept",
    vary: "origin",
  };
}

function json(res, status, body, extra = {}) {
  const data = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    ...extra,
  });
  res.end(data);
}

async function pcc(method, pathname, { key, signup, body, idem } = {}) {
  const headers = { accept: "application/json" };
  if (key) headers.authorization = `Bearer ${key}`;
  if (signup) headers["x-signup-token"] = signup;
  if (body) headers["content-type"] = "application/json";
  if (idem) headers["idempotency-key"] = idem;
  const res = await fetch(`${API}${pathname}`, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let parsed = null;
  if (text) {
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = { error: text };
    }
  }
  if (!res.ok) {
    const err = new Error(parsed?.error ?? parsed?.message ?? `API ${res.status}`);
    err.status = res.status;
    err.code = parsed?.code;
    throw err;
  }
  return parsed;
}

async function signupHarborline(invite) {
  const out = await pcc("POST", "/v1/merchants", {
    signup: invite,
    body: { name: "Harborline", payout: PAYOUT },
  });
  merchantKey = out.api_key;
  merchantName = out.name;
  saveKey(merchantKey);
  process.stderr.write("Harborline signed up. API key held in this process (and .data/, gitignored).\n");
}

async function refreshFromApi(id) {
  const intent = await pcc("GET", `/v1/payment-intents/${id}`, { key: merchantKey });
  const row = upsertOrder({
    id: intent.id,
    merchant_order_id: intent.merchant_order_id,
    status: intent.status,
    output_amount: intent.output_amount,
    checkout_url: intent.checkout_url ?? "",
    checkout_session_id: intent.checkout_session_id ?? "",
    expires_at: intent.expires_at,
    paid_at: intent.status === "succeeded" ? new Date().toISOString() : undefined,
  });
  if (intent.status === "succeeded") enqueueIfPaid(row, upsertOrder);
  return findOrder({ order: intent.merchant_order_id }) ?? row;
}

async function boot() {
  const invite = process.env.MERCHANT_SIGNUP_TOKEN?.trim() ?? "";
  if (!merchantKey) merchantKey = loadSavedKey();
  if (!merchantKey && invite) await signupHarborline(invite);
  if (!merchantKey) {
    process.stderr.write("set MERCHANT_API_KEY or MERCHANT_SIGNUP_TOKEN (never ship either to a browser)\n");
    process.exit(2);
  }
  try {
    await pcc("PUT", "/v1/payout-config", { key: merchantKey, body: PAYOUT });
  } catch (e) {
    if (e.status === 401 && invite) {
      await signupHarborline(invite);
      await pcc("PUT", "/v1/payout-config", { key: merchantKey, body: PAYOUT });
    } else {
      throw e;
    }
  }
  const me = await pcc("GET", "/v1/me", { key: merchantKey }).catch(() => ({ name: merchantName }));
  merchantName = me.name ?? merchantName;
  loadOrders();
  if (![...orders.values()].some((o) => o.merchant_order_id === LIVE_SETTLEMENT.merchant_order_id)) {
    upsertOrder(LIVE_SETTLEMENT);
  }
  persistOrders();
  if ((process.env.USE_LOCAL_PRINTER ?? "") === "1") {
    startWorker(
      () => [...orders.values()],
      upsertOrder,
    );
  } else {
    process.stderr.write("Prints are handled by printer-agent.mjs in Terminal.app (set USE_LOCAL_PRINTER=1 to print from this process).\n");
  }
  const p = printerView();
  process.stderr.write(
    `Harborline shop ${SHOP_ORIGIN} merchant=${merchantName} payout=Arbitrum USDC amount=0.20 orders=${orders.size} printer=${p.online ? "online" : "offline"}\n`,
  );
}

function serveStatic(req, res) {
  let urlPath = decodeURIComponent(new URL(req.url, SHOP_ORIGIN).pathname);
  if (urlPath === "/") urlPath = "/index.html";
  const file = path.normalize(path.join(publicDir, urlPath));
  if (!file.startsWith(publicDir)) {
    res.writeHead(403);
    res.end();
    return;
  }
  fs.readFile(file, (err, buf) => {
    if (err) {
      res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      res.end("Not found");
      return;
    }
    res.writeHead(200, { "content-type": MIME[path.extname(file)] ?? "application/octet-stream" });
    res.end(buf);
  });
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, SHOP_ORIGIN);
    const cors = corsHeaders(req);
    if (req.method === "OPTIONS" && url.pathname.startsWith("/api/")) {
      res.writeHead(204, cors);
      res.end();
      return;
    }
    if (req.method === "GET" && url.pathname === "/health") {
      json(res, 200, {
        ok: true,
        merchant: merchantName,
        amount: "0.20",
        asset: "USDC",
        chain: "arbitrum",
        orders: orders.size,
        printer: printerView(),
      });
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/printer") {
      json(res, 200, printerView(), cors);
      return;
    }
    if (req.method === "POST" && url.pathname === "/api/printer/stop") {
      json(res, 200, { ...(await stopPrint()), printer: printerView() }, cors);
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/printer/camera.jpg") {
      const jpeg = await cameraJpeg();
      if (!jpeg) {
        res.writeHead(503, { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store", ...cors });
        res.end("camera unavailable");
        return;
      }
      res.writeHead(200, {
        "content-type": "image/jpeg",
        "cache-control": "no-store",
        "content-length": String(jpeg.length),
        ...cors,
      });
      res.end(jpeg);
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/payout") {
      json(
        res,
        200,
        {
          merchant: merchantName,
          ...PAYOUT,
        },
        cors,
      );
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/orders") {
      loadOrders();
      json(
        res,
        200,
        { orders: [...orders.values()].map(publicOrder).sort((a, b) => String(b.paid_at ?? "").localeCompare(String(a.paid_at ?? ""))) },
        cors,
      );
      return;
    }
    if (req.method === "POST" && url.pathname === "/api/checkout") {
      const orderId = `boat-${Date.now()}`;
      const intent = await pcc("POST", "/v1/payment-intents", {
        key: merchantKey,
        idem: `${orderId}-${crypto.randomUUID()}`,
        body: {
          merchant_order_id: orderId,
          output_amount: AMOUNT,
          success_url: SUCCESS_URL,
        },
      });
      const row = upsertOrder({
        id: intent.id,
        merchant_order_id: orderId,
        status: intent.status ?? "requires_payment",
        output_amount: intent.output_amount ?? AMOUNT,
        checkout_url: intent.checkout_url ?? "",
        checkout_session_id: intent.checkout_session_id ?? "",
        expires_at: intent.expires_at,
      });
      const session = intent.checkout_session_id ?? "";
      json(res, 200, {
        order_id: orderId,
        intent_id: intent.id,
        checkout_session_id: session,
        checkout_url: intent.checkout_url,
        x402_url: `${SHOP_ORIGIN}/x402.html?session=${encodeURIComponent(session)}&intent=${encodeURIComponent(intent.id)}&order=${encodeURIComponent(orderId)}`,
        amount: "0.20",
        asset: "USDC",
        order: publicOrder(row),
      });
      return;
    }
    if (url.pathname === "/api/x402") {
      const action = url.searchParams.get("action") || "create";
      const session = url.searchParams.get("session") || "";
      if (action === "supported" && req.method === "GET") {
        json(res, 200, await pcc("GET", "/v1/x402/supported"), cors);
        return;
      }
      if (action === "requirements" && req.method === "GET") {
        if (!session) {
          json(res, 400, { error: "session required" }, cors);
          return;
        }
        json(res, 200, await pcc("GET", `/v1/x402/sessions/${session}/requirements`), cors);
        return;
      }
      if (action === "prepare" && req.method === "POST") {
        if (!session) {
          json(res, 400, { error: "session required" }, cors);
          return;
        }
        const body = JSON.parse((await readBody(req)) || "{}");
        json(res, 200, await pcc("POST", `/v1/x402/sessions/${session}/prepare`, { body }), cors);
        return;
      }
      if (action === "verify" && req.method === "POST") {
        const body = JSON.parse((await readBody(req)) || "{}");
        json(res, 200, await pcc("POST", "/v1/x402/verify", { body }), cors);
        return;
      }
      if (action === "settle" && req.method === "POST") {
        const body = JSON.parse((await readBody(req)) || "{}");
        json(res, 200, await pcc("POST", "/v1/x402/settle", { body }), cors);
        return;
      }
      json(res, 405, { error: "unsupported x402 action" }, cors);
      return;
    }
    if (req.method === "GET" && url.pathname === "/api/order") {
      const intentId = url.searchParams.get("intent");
      const orderId = url.searchParams.get("order");
      const sessionId = url.searchParams.get("session");
      let row = findOrder({ intent: intentId, order: orderId, session: sessionId });
      if (intentId) {
        try {
          row = await refreshFromApi(intentId);
        } catch (e) {
          if (!row) throw e;
        }
      }
      if (!row) {
        json(res, 404, { error: "order not found" }, cors);
        return;
      }
      json(res, 200, publicOrder(row), cors);
      return;
    }
    if (req.method === "POST") {
      await readBody(req);
      json(res, 404, { error: "not found" });
      return;
    }
    if (req.method === "GET") {
      serveStatic(req, res);
      return;
    }
    res.writeHead(405);
    res.end();
  } catch (e) {
    json(res, e.status && e.status < 500 ? e.status : 502, { error: e.message, code: e.code });
  }
});

await boot();
server.listen(PORT, HOST);
