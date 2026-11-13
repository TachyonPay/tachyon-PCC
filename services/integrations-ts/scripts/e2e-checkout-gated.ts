/**
 * Local E2E through pcc-api hosted checkout: intent → SIWE (Wallet A signs a
 * login message) → allowance (Wallet A signs the EIP-712 enable once, reused by
 * later payments) → TOTP → authorize. The first authorize asks to remember the
 * device, so later payments resume with that token instead of a SIWE signature.
 * No signature is a transaction. Expects a real 1Click
 * executable quote and `execution: "gated"` while DRY_RUN=true. Never prints
 * keys, tokens, or the TOTP secret.
 */
import { resolve } from "node:path";
import { loadRepoEnv } from "./_env.js";
import { base32Decode, totp } from "./_totp.js";
import { loadSessionSigner } from "../src/account-abstraction/keys.js";

loadRepoEnv();
const API = process.env["E2E_API"] ?? "http://127.0.0.1:8080";
const MERCHANT_KEY = process.env["E2E_MERCHANT_KEY"] ?? "pcc_sk_alice_demo";
const OUTPUT = process.env["E2E_OUTPUT"] ?? "200000";
const ownerPath = process.env["SOURCE_EXECUTOR_KEY_REF"]?.trim() || resolve(process.cwd(), "../../secrets/wallet-a.key");
const owner = loadSessionSigner(ownerPath);

async function call(method: string, path: string, body?: unknown, headers: Record<string, string> = {}) {
  const res = await fetch(`${API}${path}`, {
    method,
    headers: { accept: "application/json", ...(body ? { "content-type": "application/json" } : {}), ...headers },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const text = await res.text();
  const json = text ? JSON.parse(text) : null;
  if (!res.ok) throw new Error(`${method} ${path} -> ${res.status} ${text}`);
  return json;
}

const payoutTo = process.env["E2E_PAYOUT_TO"]?.trim() || process.env["WALLET_B_ADDRESS"]?.trim();
if (payoutTo) {
  await call(
    "PUT",
    "/v1/payout-config",
    {
      receiving_address: payoutTo,
      chain_id: 42161,
      token_address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831",
      decimals: 6,
      one_click_asset_id: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near",
    },
    { authorization: `Bearer ${MERCHANT_KEY}` },
  );
  console.log(JSON.stringify({ phase: "payout", receivingAddress: payoutTo }));
}

const payments = Math.max(1, Number(process.env["E2E_PAYMENTS"] ?? "1") || 1);
const expected = process.env["E2E_EXPECT_EXECUTION"]?.trim() || "gated";
const sourceAsset = process.env["E2E_SOURCE_ASSET"]?.trim();
let secret: Buffer | undefined;
let lastStep = -1;
let signatures = 0;
let siweSignatures = 0;
let deviceToken: string | undefined;

for (let n = 1; n <= payments; n++) {
  const intent = await call(
    "POST",
    "/v1/payment-intents",
    { merchant_order_id: `e2e-${Date.now()}`, output_amount: OUTPUT },
    { authorization: `Bearer ${MERCHANT_KEY}`, "idempotency-key": `e2e-${Date.now()}` },
  );
  const sid = intent.checkout_session_id as string;
  let verified;
  if (deviceToken) {
    verified = await call("POST", `/v1/checkout/${sid}/device/resume`, { device_token: deviceToken });
    if (verified.address.toLowerCase() !== owner.address.toLowerCase()) throw new Error("device resumed a different wallet");
  } else {
    const challenge = await call("POST", `/v1/checkout/${sid}/siwe/challenge`, { address: owner.address });
    const signature = await owner.signMessage({ message: challenge.message });
    verified = await call("POST", `/v1/checkout/${sid}/siwe/verify`, { message: challenge.message, signature });
    siweSignatures++;
  }
  console.log(
    JSON.stringify({
      phase: deviceToken ? "device_resume" : "siwe",
      payment: n,
      totpEnrolled: verified.totp_enrolled,
      quotedInput: verified.quoted_input,
      maxDebit: verified.max_debit,
    }),
  );

  if (sourceAsset) {
    const picked = await call("POST", `/v1/checkout/${sid}/source-asset`, { asset_id: sourceAsset });
    console.log(JSON.stringify({ phase: "source", sourceAsset: picked.source_asset, quotedInput: picked.quoted_input, maxDebit: picked.max_debit }));
  }

  // One payer-signed allowance serves every payment: per-payment cap, uses, expiry.
  // It is an EIP-712 signature, not a transaction; gates decide whether it is ever used.
  const statusRes = await fetch(`${API}/v1/checkout/${sid}/session-grant`);
  type AllowanceView = { covers_this_payment: boolean; uses_remaining: number };
  let allowance = statusRes.ok ? ((await statusRes.json()) as AllowanceView) : null;
  if (!statusRes.ok && statusRes.status !== 404) throw new Error(`allowance status -> ${statusRes.status}`);
  if (allowance?.covers_this_payment) {
    console.log(JSON.stringify({ phase: "allowance", payment: n, reused: true, usesRemaining: allowance.uses_remaining }));
  } else {
    if (signatures > 0) throw new Error("payment needed a second allowance signature; one should cover all payments");
    const cap = process.env["E2E_ALLOWANCE_CAP"]?.trim() || (BigInt(verified.max_debit) * 2n).toString();
    const grant = await call("POST", `/v1/checkout/${sid}/session-grant`, {
      max_amount: cap,
      uses: Math.min(20, payments),
      valid_days: 1,
    });
    const grantSig = await owner.signTypedData(grant.typed_data);
    allowance = (await call("POST", `/v1/checkout/${sid}/session-grant/enable`, { grant_id: grant.grant_id, signature: grantSig })) as AllowanceView;
    signatures++;
    console.log(
      JSON.stringify({
        phase: "allowance",
        payment: n,
        reused: false,
        chainId: grant.chain_id,
        maxAmount: grant.max_amount,
        uses: grant.uses,
        totalExposure: grant.total_exposure,
      }),
    );
  }
  const usesBefore = allowance.uses_remaining;

  if (!secret) {
    if (verified.totp_enrolled) {
      throw new Error("Wallet A already has TOTP in this API process and this script never stores the secret; restart pcc-api and rerun.");
    }
    const enroll = await call("POST", `/v1/checkout/${sid}/totp/enroll`, {});
    secret = base32Decode(new URL(enroll.otpauth_uri).searchParams.get("secret") ?? "");
    await call("POST", `/v1/checkout/${sid}/totp/confirm`, { code: totp(secret) });
    console.log(JSON.stringify({ phase: "enroll", payment: n }));
  } else {
    const enrollAgain = await fetch(`${API}/v1/checkout/${sid}/totp/enroll`, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
    if (enrollAgain.status !== 409) throw new Error(`re-enrollment should be refused, got ${enrollAgain.status}`);
  }

  // A code is accepted once per 30 s step, so later payments wait for a fresh one.
  while (Math.floor(Date.now() / 30_000) === lastStep) await new Promise((r) => setTimeout(r, 1_000));
  lastStep = Math.floor(Date.now() / 30_000);
  const auth = await call("POST", `/v1/checkout/${sid}/authorize`, {
    code: totp(secret),
    remember_device: !deviceToken,
  });
  if (auth.device_token) deviceToken = auth.device_token as string;
  console.log(
    JSON.stringify({
      phase: "authorize",
      payment: n,
      status: auth.status,
      execution: auth.execution,
      simulationOk: auth.simulation_ok,
      quotedInput: auth.quoted_input,
      depositAddress: auth.deposit_address,
      deviceRemembered: !!deviceToken,
    }),
  );
  if (auth.execution !== expected) process.exit(1);
  if (auth.execution === "gated") {
    const after = await call("GET", `/v1/checkout/${sid}/session-grant`);
    if (after.uses_remaining !== usesBefore) throw new Error("a gated payment must not spend an allowance use");
  }
}
if (deviceToken) {
  const forgot = await call("POST", "/v1/devices/forget", { device_token: deviceToken });
  if (!forgot.forgotten) throw new Error("device token should have been forgettable");
}
console.log(JSON.stringify({ phase: "summary", payments, allowanceSignatures: signatures, siweSignatures }));
