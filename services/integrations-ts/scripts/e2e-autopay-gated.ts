/**
 * Local autopay E2E through pcc-api: a hosted checkout signs Wallet A in,
 * enrolls TOTP, and sets up a mandate (TOTP code + one EIP-712 signature).
 * The merchant then charges with its API key only, with no payer step. While
 * DRY_RUN=true each charge must end `gated` with a passing simulation; nothing
 * is sent. No signature here is a transaction. Never prints keys, tokens or
 * the TOTP secret.
 *
 * Needs a freshly started pcc-api (Wallet A not yet enrolled in that process).
 */
import { randomUUID } from "node:crypto";
import { resolve } from "node:path";
import { loadRepoEnv } from "./_env.js";
import { base32Decode, nextStep, totp } from "./_totp.js";
import { loadSessionSigner } from "../src/account-abstraction/keys.js";

loadRepoEnv();
const API = process.env["E2E_API"] ?? "http://127.0.0.1:8080";
const MERCHANT_KEY = process.env["E2E_MERCHANT_KEY"] ?? "pcc_sk_alice_demo";
const OUTPUT = process.env["E2E_OUTPUT"] ?? "200000";
const CHARGES = Math.min(20, Math.max(1, Number(process.env["E2E_CHARGES"] ?? "2") || 2));
const ownerPath = process.env["SOURCE_EXECUTOR_KEY_REF"]?.trim() || resolve(process.cwd(), "../../secrets/wallet-a.key");
const owner = loadSessionSigner(ownerPath);
const merchant = { authorization: `Bearer ${MERCHANT_KEY}` };

async function send(method: string, path: string, body?: unknown, headers: Record<string, string> = {}) {
  const res = await fetch(`${API}${path}`, {
    method,
    headers: { accept: "application/json", ...(body ? { "content-type": "application/json" } : {}), ...headers },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const text = await res.text();
  return { status: res.status, json: text ? JSON.parse(text) : null, text };
}

async function call(method: string, path: string, body?: unknown, headers: Record<string, string> = {}) {
  const r = await send(method, path, body, headers);
  if (r.status < 200 || r.status >= 300) throw new Error(`${method} ${path} -> ${r.status} ${r.text}`);
  return r.json;
}

const intent = await call(
  "POST",
  "/v1/payment-intents",
  { merchant_order_id: `autopay-setup-${Date.now()}`, output_amount: OUTPUT },
  { ...merchant, "idempotency-key": randomUUID() },
);
const sid = intent.checkout_session_id as string;
const challenge = await call("POST", `/v1/checkout/${sid}/siwe/challenge`, { address: owner.address });
const verified = await call("POST", `/v1/checkout/${sid}/siwe/verify`, {
  message: challenge.message,
  signature: await owner.signMessage({ message: challenge.message }),
});
if (verified.totp_enrolled) {
  throw new Error("Wallet A already has TOTP in this API process and this script never stores the secret; restart pcc-api and rerun.");
}
const enroll = await call("POST", `/v1/checkout/${sid}/totp/enroll`, {});
const secret = base32Decode(new URL(enroll.otpauth_uri).searchParams.get("secret") ?? "");
let step = Math.floor(Date.now() / 30_000);
await call("POST", `/v1/checkout/${sid}/totp/confirm`, { code: totp(secret) });
console.log(JSON.stringify({ phase: "enroll", maxDebit: verified.max_debit }));

step = await nextStep(step);
const cap = process.env["E2E_AUTOPAY_CAP"]?.trim() || (BigInt(verified.max_debit) * 2n).toString();
const setup = await call("POST", `/v1/checkout/${sid}/autopay`, {
  code: totp(secret),
  max_amount: cap,
  charges: CHARGES,
  interval_hours: 0,
  valid_days: 1,
});
const enabled = await call("POST", `/v1/checkout/${sid}/autopay/${setup.mandate_id}/enable`, {
  grant_id: setup.grant_id,
  signature: await owner.signTypedData(setup.typed_data),
});
console.log(
  JSON.stringify({
    phase: "mandate",
    status: enabled.status,
    chainId: setup.chain_id,
    maxAmount: setup.max_amount,
    charges: setup.charges,
    totalExposure: setup.total_exposure,
  }),
);
if (enabled.status !== "active") process.exit(1);

for (let n = 1; n <= CHARGES; n++) {
  const charge = await call(
    "POST",
    `/v1/autopay/${setup.mandate_id}/charges`,
    { merchant_order_id: `autopay-${n}-${Date.now()}`, output_amount: OUTPUT },
    { ...merchant, "idempotency-key": randomUUID() },
  );
  console.log(
    JSON.stringify({
      phase: "charge",
      charge: n,
      execution: charge.execution,
      simulationOk: charge.simulation_ok,
      quotedInput: charge.quoted_input,
      depositAddress: charge.deposit_address,
      chargesRemaining: charge.charges_remaining,
    }),
  );
  if (charge.execution !== "gated" || charge.simulation_ok !== true) process.exit(1);
}

const extra = await send(
  "POST",
  `/v1/autopay/${setup.mandate_id}/charges`,
  { merchant_order_id: `autopay-extra-${Date.now()}`, output_amount: OUTPUT },
  { ...merchant, "idempotency-key": randomUUID() },
);
console.log(JSON.stringify({ phase: "over_limit", status: extra.status, code: extra.json?.code }));
if (extra.status !== 409 || extra.json?.code !== "mandate_exhausted") process.exit(1);

const revoked = await call("POST", `/v1/checkout/${sid}/autopay/${setup.mandate_id}/revoke`, {});
console.log(JSON.stringify({ phase: "revoke", status: revoked.status, payerSignatures: 2, merchantCharges: CHARGES }));
