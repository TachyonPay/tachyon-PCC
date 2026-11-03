/**
 * LIVE read-only call: POST /v0/quote with dry:true, EXACT_OUTPUT,
 * confidentiality:"basic", then official signature verification.
 *
 * - Requires ONECLICK_API_KEY in the repo-root .env (never printed).
 * - dry:true => 1Click allocates NO deposit address; nothing can be funded.
 * - Recipient/refund default to placeholder addresses (no funds involved).
 *
 * Usage:
 *   pnpm live:dry-quote                      # Base USDC -> Arbitrum USDC, 1.00 USDC out
 *   ORIGIN=<assetId> DEST=<assetId> AMOUNT_OUT=<baseUnits> pnpm live:dry-quote
 */
import { loadRepoEnv } from "./_env.js";
import { OneClickClient } from "../src/near/client.js";
import { NearService } from "../src/near/service.js";
import { WorkerError } from "../src/errors.js";

loadRepoEnv();
const apiKey = process.env["ONECLICK_API_KEY"]?.trim();
if (!apiKey) {
  console.error("ONECLICK_API_KEY not set in .env — refusing to call 1Click.");
  process.exit(2);
}

const ORIGIN = process.env["ORIGIN"] ?? "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near";
const DEST = process.env["DEST"] ?? "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near";
const AMOUNT_OUT = process.env["AMOUNT_OUT"] ?? "1000000";
const CONF = (process.env["CONFIDENTIALITY"] ?? "basic") as "public" | "basic" | "advanced";
// Placeholder, non-zero, obviously-not-ours addresses: dry quotes never fund anything.
const RECIPIENT = process.env["RECIPIENT"] ?? "0x1111111111111111111111111111111111111111";
const REFUND = process.env["REFUND"] ?? "0x2222222222222222222222222222222222222222";

const client = new OneClickClient({ baseUrl: process.env["ONECLICK_BASE_URL"] ?? "https://1click.chaindefuser.com", apiKey, timeoutMs: 25_000 });
const near = new NearService(client);

try {
  const q = await near.quote({
    dry: true,
    authorizationRef: `live-dry-${Date.now()}`,
    originAssetId: ORIGIN,
    destinationAssetId: DEST,
    amountOut: AMOUNT_OUT,
    recipient: RECIPIENT,
    refundTo: REFUND,
    confidentiality: CONF,
    slippageBps: 100,
    deadline: new Date(Date.now() + 60 * 60_000).toISOString(),
  });
  const { rawResponseJson: _raw, ...redacted } = q;
  console.log("DRY CONFIDENTIAL QUOTE (signature verified by official verifier):");
  console.log(JSON.stringify(redacted, null, 2));
  console.log(`\nslippage buffer: amountIn ${q.amountIn} vs minAmountIn ${q.minAmountIn} for exact amountOut ${q.amountOut}`);
  console.log(`depositAddress present: ${q.depositAddress !== undefined} (expected false for dry)`);
} catch (e) {
  if (e instanceof WorkerError) {
    console.error(`FAILED ${e.code} (${e.httpStatus}): ${e.message}`);
    process.exit(1);
  }
  throw e;
}
