/**
 * Harborline → pay.tachyon.pe. Merchant key stays on the server / laptop agent.
 * Never import this file from public/*.js.
 */
export const API = process.env.API_BASE_URL ?? "https://pay.tachyon.pe";
export const AMOUNT = process.env.AMOUNT_OUT ?? "200000";
export const PAYOUT = {
  receiving_address: process.env.WALLET_B_ADDRESS ?? "0x049FdEf2550772B9F36F86757C27f5F982D0970D",
  chain_id: 42161,
  decimals: 6,
  one_click_asset_id: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near",
  token_address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831",
};

export function shopOrigin(req) {
  if (process.env.SHOP_ORIGIN) return process.env.SHOP_ORIGIN.replace(/\/$/, "");
  const proto = String(req?.headers?.["x-forwarded-proto"] || "https").split(",")[0].trim();
  let host = String(req?.headers?.["x-forwarded-host"] || req?.headers?.host || process.env.VERCEL_PROJECT_PRODUCTION_URL || process.env.VERCEL_URL || "").split(",")[0].trim();
  if (host.startsWith("http://") || host.startsWith("https://")) return host.replace(/\/$/, "");
  if (host) return `${proto === "http" ? "http" : "https"}://${host}`.replace(/\/$/, "");
  return "http://127.0.0.1:3002";
}

export function merchantKey() {
  const key = process.env.MERCHANT_API_KEY?.trim();
  if (!key) {
    const err = new Error("MERCHANT_API_KEY missing");
    err.status = 500;
    throw err;
  }
  return key;
}

export async function pcc(method, pathname, { key, body, idem } = {}) {
  const headers = { accept: "application/json" };
  if (key) headers.authorization = `Bearer ${key}`;
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

const PAY_CHAIN = {
  eth: "Ethereum",
  op: "Optimism",
  base: "Base",
  arb: "Arbitrum",
  1: "Ethereum",
  10: "Optimism",
  8453: "Base",
  42161: "Arbitrum",
};

function payChainFromSourceAsset(assetId) {
  const s = String(assetId || "");
  const slug = /nep141:(eth|op|base|arb)-/i.exec(s);
  if (slug) return PAY_CHAIN[slug[1].toLowerCase()];
  const cid = /:(1|10|8453|42161)_/.exec(s);
  return cid ? PAY_CHAIN[cid[1]] : null;
}

export async function publicIntent(intent) {
  let pay_chain = null;
  let quoted_input = null;
  let deposit_address = null;
  const session = intent.checkout_session_id;
  if (session) {
    try {
      const view = await pcc("GET", `/v1/checkout/${session}`);
      pay_chain = payChainFromSourceAsset(view.source_asset);
      quoted_input = view.quoted_input ?? null;
      deposit_address = view.deposit_address ?? null;
    } catch {
      /* receipt still renders without the pay-in chain */
    }
  }
  return {
    id: intent.id,
    merchant_id: intent.merchant_id ?? "harborline",
    merchant_order_id: intent.merchant_order_id,
    product: "Mini Hull",
    status: intent.status,
    output_amount: intent.output_amount,
    quoted_input,
    deposit_address,
    checkout_session_id: session ?? "",
    checkout_url: intent.checkout_url ?? "",
    expires_at: intent.expires_at,
    pay_chain: pay_chain || "the pay-in chain",
    settle_chain: "Arbitrum",
    pay_asset: "USDC",
    settle_asset: "USDC",
    receiving_address: PAYOUT.receiving_address,
    print: null,
    explorers: {
      base: null,
      arbitrum: null,
      confidential: `/confidential.html?order=${encodeURIComponent(intent.merchant_order_id ?? "")}`,
    },
  };
}

export async function createCheckout(req) {
  const key = merchantKey();
  const origin = shopOrigin(req);
  await pcc("PUT", "/v1/payout-config", { key, body: PAYOUT }).catch(() => null);
  const orderId = `boat-${Date.now()}`;
  const intent = await pcc("POST", "/v1/payment-intents", {
    key,
    idem: `${orderId}-${crypto.randomUUID()}`,
    body: {
      merchant_order_id: orderId,
      output_amount: AMOUNT,
      success_url: `${origin}/return.html`,
    },
  });
  const session = intent.checkout_session_id ?? "";
  return {
    order_id: orderId,
    intent_id: intent.id,
    checkout_session_id: session,
    checkout_url: intent.checkout_url,
    x402_url: `${origin}/x402.html?session=${encodeURIComponent(session)}&intent=${encodeURIComponent(intent.id)}&order=${encodeURIComponent(orderId)}`,
    amount: "0.20",
    asset: "USDC",
    order: await publicIntent(intent),
  };
}

/** Public x402 facilitator proxy (no merchant key on these PCC routes). */
export async function x402Requirements(sessionId) {
  return pcc("GET", `/v1/x402/sessions/${sessionId}/requirements`);
}

export async function x402Prepare(sessionId, body) {
  return pcc("POST", `/v1/x402/sessions/${sessionId}/prepare`, { body });
}

export async function x402Verify(body) {
  return pcc("POST", `/v1/x402/verify`, { body });
}

export async function x402Settle(body) {
  return pcc("POST", `/v1/x402/settle`, { body });
}

export async function x402Supported() {
  return pcc("GET", "/v1/x402/supported");
}

export async function listShopOrders() {
  const key = merchantKey();
  const intents = await pcc("GET", "/v1/payment-intents", { key });
  const rows = Array.isArray(intents) ? intents : [];
  return (await Promise.all(rows.map(publicIntent))).sort((a, b) =>
    String(b.expires_at ?? "").localeCompare(String(a.expires_at ?? "")),
  );
}

export async function getShopOrder({ intent, order, session }) {
  const key = merchantKey();
  if (intent) {
    const row = await pcc("GET", `/v1/payment-intents/${intent}`, { key });
    return await publicIntent(row);
  }
  const rows = await listShopOrders();
  return rows.find((r) => (order && r.merchant_order_id === order) || (session && r.checkout_session_id === session)) ?? null;
}
