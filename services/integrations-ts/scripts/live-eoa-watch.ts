/**
 * After an EOA deposit is broadcast: optional /v0/deposit/submit, then poll
 * 1Click /v0/status. Never loads a private key. Never prints the API key.
 *
 * Env: DEPOSIT_ADDRESS, optional SOURCE_TX_HASH, ONECLICK_API_KEY.
 */
import { loadRepoEnv } from "./_env.js";
import { OneClickClient } from "../src/near/client.js";
import { WorkerError } from "../src/errors.js";
import { isAddress, isHash } from "viem";

loadRepoEnv();
const apiKey = process.env["ONECLICK_API_KEY"]?.trim();
if (!apiKey) {
  console.error("ONECLICK_API_KEY not set — refusing.");
  process.exit(2);
}

const deposit = (process.env["DEPOSIT_ADDRESS"] ?? "").trim();
const txHash = (process.env["SOURCE_TX_HASH"] ?? "").trim();
if (!isAddress(deposit)) {
  console.error("DEPOSIT_ADDRESS must be an EVM address.");
  process.exit(2);
}
if (txHash && !isHash(txHash)) {
  console.error("SOURCE_TX_HASH must be a 0x-prefixed 32-byte hash when set.");
  process.exit(2);
}

const client = new OneClickClient({
  baseUrl: process.env["ONECLICK_BASE_URL"] ?? "https://1click.chaindefuser.com",
  apiKey,
  timeoutMs: 25_000,
});

const polls = Number(process.env["STATUS_POLLS"] ?? "24");
const delayMs = Number(process.env["STATUS_DELAY_MS"] ?? "5000");

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

try {
  if (txHash) {
    try {
      await client.submitDepositTx(deposit, txHash);
      console.log(JSON.stringify({ phase: "submit", ok: true, depositAddress: deposit, txHash }));
    } catch (e) {
      const msg = e instanceof WorkerError ? e.message : e instanceof Error ? e.message : String(e);
      console.log(JSON.stringify({ phase: "submit", ok: false, error: msg }));
    }
  }

  for (let i = 0; i < polls; i += 1) {
    const status = await client.getStatus(deposit);
    const redacted = {
      phase: "status",
      poll: i,
      status: status.status,
      updatedAt: status.updatedAt,
      originChainTxHashes: status.swapDetails.originChainTxHashes.map((t) => t.hash),
      destinationChainTxHashes: status.swapDetails.destinationChainTxHashes.map((t) => t.hash),
      amountIn: status.swapDetails.amountIn,
      amountOut: status.swapDetails.amountOut,
      refundedAmount: status.swapDetails.refundedAmount,
    };
    console.log(JSON.stringify(redacted));
    const done = new Set(["SUCCESS", "INCOMPLETE_DEPOSIT", "REFUNDED", "FAILED"]);
    if (done.has(status.status)) {
      process.exit(status.status === "SUCCESS" ? 0 : 1);
    }
    await sleep(delayMs);
  }
  console.error("status still pending after bounded polls");
  process.exit(2);
} catch (e) {
  if (e instanceof WorkerError) {
    console.error(`FAILED ${e.code} (${e.httpStatus}): ${e.message}`);
    process.exit(1);
  }
  throw e;
}
