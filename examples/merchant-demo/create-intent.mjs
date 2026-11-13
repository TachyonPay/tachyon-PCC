#!/usr/bin/env node
// Tiny merchant backend sample. Holds an API key on the server side only.
const base = process.env.API_BASE_URL ?? "http://127.0.0.1:8080";
const key = process.env.MERCHANT_API_KEY;
if (!key) {
  console.error("set MERCHANT_API_KEY (never ship this to a browser)");
  process.exit(2);
}
const res = await fetch(`${base}/v1/payment-intents`, {
  method: "POST",
  headers: {
    authorization: `Bearer ${key}`,
    "content-type": "application/json",
    "idempotency-key": process.env.IDEMPOTENCY_KEY ?? `demo-${Date.now()}`,
  },
  body: JSON.stringify({
    merchant_order_id: process.env.ORDER_ID ?? "boat-cli",
    output_amount: process.env.AMOUNT_OUT ?? "200000",
    ...(process.env.SUCCESS_URL ? { success_url: process.env.SUCCESS_URL } : {}),
  }),
});
const body = await res.text();
if (!res.ok) {
  console.error(res.status, body);
  process.exit(1);
}
console.log(body);
