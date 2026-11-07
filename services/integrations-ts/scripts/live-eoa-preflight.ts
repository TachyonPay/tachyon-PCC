/**
 * LIVE preflight for the M2 EOA deposit. Does NOT sign or broadcast.
 *
 * 1. POST /v0/quote dry:false (allocates a 1Click deposit address + deadline)
 * 2. Official signature verify + echo-binding
 * 3. eth_call / estimateGas of USDC.transfer(deposit, amountIn) from WALLET_A
 *
 * Requires WALLET_A_ADDRESS, ONECLICK_API_KEY. RPC_URL_BASE defaults to
 * https://mainnet.base.org (official). Never prints the API key or any
 * private key (this script does not load SOURCE_EXECUTOR_KEY_REF).
 */
import { loadRepoEnv } from "./_env.js";
import { OneClickClient } from "../src/near/client.js";
import { NearService } from "../src/near/service.js";
import { WorkerError } from "../src/errors.js";
import { createPublicClient, encodeFunctionData, erc20Abi, http, isAddress, type Address } from "viem";
import { base } from "viem/chains";

loadRepoEnv();
const apiKey = process.env["ONECLICK_API_KEY"]?.trim();
if (!apiKey) {
  console.error("ONECLICK_API_KEY not set — refusing.");
  process.exit(2);
}

const ORIGIN = process.env["ORIGIN"] ?? "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near";
const DEST = process.env["DEST"] ?? "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near";
const AMOUNT_OUT = process.env["AMOUNT_OUT"] ?? "1000000";
const USDC_BASE = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" as Address;
const wallet = (process.env["WALLET_A_ADDRESS"] ?? "").trim();
const merchant = (process.env["WALLET_B_ADDRESS"] ?? wallet).trim();
if (!isAddress(wallet) || !isAddress(merchant)) {
  console.error("WALLET_A_ADDRESS / WALLET_B_ADDRESS must be set to EVM addresses.");
  process.exit(2);
}

const rpc = process.env["RPC_URL_BASE"]?.trim() || "https://mainnet.base.org";
const client = new OneClickClient({
  baseUrl: process.env["ONECLICK_BASE_URL"] ?? "https://1click.chaindefuser.com",
  apiKey,
  timeoutMs: 25_000,
});
const near = new NearService(client);

try {
  const q = await near.quote({
    dry: false,
    authorizationRef: `eoa-preflight-${Date.now()}`,
    originAssetId: ORIGIN,
    destinationAssetId: DEST,
    amountOut: AMOUNT_OUT,
    recipient: merchant,
    refundTo: wallet,
    confidentiality: "basic",
    slippageBps: 100,
    deadline: new Date(Date.now() + 60 * 60_000).toISOString(),
  });
  if (!q.depositAddress) {
    console.error("executable quote missing depositAddress — refusing to continue.");
    process.exit(1);
  }
  const { rawResponseJson: _raw, ...redacted } = q;
  console.log("EXECUTABLE QUOTE (verified; no broadcast):");
  console.log(JSON.stringify(redacted, null, 2));

  const evm = createPublicClient({ chain: base, transport: http(rpc) });
  const data = encodeFunctionData({
    abi: erc20Abi,
    functionName: "transfer",
    args: [q.depositAddress as Address, BigInt(q.amountIn)],
  });
  const from = wallet as Address;
  try {
    await evm.call({ account: from, to: USDC_BASE, data });
    const gas = await evm.estimateGas({ account: from, to: USDC_BASE, data });
    const fees = await evm.estimateFeesPerGas();
    const maxFee = gas * (fees.maxFeePerGas ?? 0n);
    console.log("\nSIMULATION (eth_call + estimateGas; unsigned):");
    console.log(
      JSON.stringify(
        {
          ok: true,
          token: USDC_BASE,
          from,
          depositAddress: q.depositAddress,
          amountIn: q.amountIn,
          gasEstimate: gas.toString(),
          maxFeePerGasWei: (fees.maxFeePerGas ?? 0n).toString(),
          maxFeeWei: maxFee.toString(),
          maxFeeEth: Number(maxFee) / 1e18,
        },
        null,
        2,
      ),
    );
  } catch (simErr) {
    console.log("\nSIMULATION FAILED (no broadcast attempted):", simErr instanceof Error ? simErr.message : simErr);
    process.exit(1);
  }
  console.log("\nSTOP. Reply with an approval to broadcast THIS deposit (chain Base, token USDC, amountIn above, this depositAddress).");
} catch (e) {
  if (e instanceof WorkerError) {
    console.error(`FAILED ${e.code} (${e.httpStatus}): ${e.message}`);
    process.exit(1);
  }
  throw e;
}
