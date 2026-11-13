/**
 * M4 live: one Base USDC → Arbitrum USDC 1Click deposit, signed by the
 * session key on the already-7702 Wallet A. No second 7702 install.
 *
 * Owner-approved in chat: 0.20 USDC out. Never prints keys or the API key.
 * Process gates: DRY_RUN=false ENABLE_MAINNET_TRANSFERS=true
 */
import { resolve } from "node:path";
import { createPublicClient, erc20Abi, http, isAddress, type Address } from "viem";
import { arbitrum, base } from "viem/chains";
import { createKernelAccount, createKernelAccountClient } from "@zerodev/sdk";
import { toPermissionValidator } from "@zerodev/permissions";
import { toECDSASigner } from "@zerodev/permissions/signers";
import { deserializePermissionAccount, serializePermissionAccount } from "@zerodev/permissions";
import { loadRepoEnv } from "./_env.js";
import { loadSessionSigner } from "../src/account-abstraction/keys.js";
import {
  ENTRY_POINT,
  KERNEL_VERSION,
  KERNEL_7702_DELEGATION_ADDRESS,
  SUPPORTED_7702_CHAINS,
  USDC_BASE,
} from "../src/account-abstraction/pins.js";
import { encodeErc20Transfer } from "../src/account-abstraction/call.js";
import { buildGrant, buildSessionPolicies } from "../src/account-abstraction/grant.js";
import { assertOnboardAllowed, validateSessionCall } from "../src/account-abstraction/validate.js";
import { checkBroadcast, gatesFromEnv } from "../src/account-abstraction/gates.js";
import { OneClickClient } from "../src/near/client.js";
import { NearService } from "../src/near/service.js";
import { WorkerError } from "../src/errors.js";

loadRepoEnv();

const AMOUNT_OUT = BigInt(process.env["AMOUNT_OUT"] ?? "200000"); // 0.20 USDC
const APPROVAL_REF = process.env["APPROVAL_REF"]?.trim() || "chat-base-7702-0.20-usdc-out";
const MAX_GAS_WEI = BigInt(process.env["MAX_GAS_WEI"] ?? "300000000000000"); // 0.0003 ETH
const ORIGIN = "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near";
const DEST = "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near";
const USDC_ARB = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831" as Address;
const OWNER_PATH =
  process.env["SOURCE_EXECUTOR_KEY_REF"]?.trim() || resolve(process.cwd(), "../../secrets/wallet-a.key");
const SESSION_PATH =
  process.env["SESSION_SIGNER_KEY_REF"]?.trim() || resolve(process.cwd(), "../../secrets/session-userop.key");
const RPC = process.env["RPC_URL_BASE"]?.trim() || "https://mainnet.base.org";
const RPC_ARB = process.env["RPC_URL_ARBITRUM"]?.trim() || "https://arb1.arbitrum.io/rpc";
const BUNDLER = process.env["BUNDLER_RPC_BASE"]?.trim() || "https://public.pimlico.io/v2/8453/rpc";
const payer = (process.env["WALLET_A_ADDRESS"] ?? "").trim();
const merchant = (process.env["WALLET_B_ADDRESS"] ?? payer).trim();

function log(phase: string, data: Record<string, unknown>): void {
  console.log(JSON.stringify({ phase, ...data }));
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

const apiKey = process.env["ONECLICK_API_KEY"]?.trim();
if (!apiKey) {
  console.error("ONECLICK_API_KEY not set — refusing.");
  process.exit(2);
}
if (!isAddress(payer) || !isAddress(merchant)) {
  console.error("WALLET_A_ADDRESS / WALLET_B_ADDRESS must be EVM addresses.");
  process.exit(2);
}

const gates = gatesFromEnv();
const gate = checkBroadcast(gates, SUPPORTED_7702_CHAINS.base, APPROVAL_REF);
if (!gate.ok) {
  console.error(
    `refusing: ${gate.reason}. Re-run with DRY_RUN=false ENABLE_MAINNET_TRANSFERS=true on this process only.`,
  );
  process.exit(2);
}

const owner = loadSessionSigner(OWNER_PATH);
const session = loadSessionSigner(SESSION_PATH);
if (owner.address.toLowerCase() !== payer.toLowerCase()) {
  console.error("wallet-a.key does not match WALLET_A_ADDRESS");
  process.exit(1);
}

const publicClient = createPublicClient({ chain: base, transport: http(RPC) });
const arbClient = createPublicClient({ chain: arbitrum, transport: http(RPC_ARB) });
const code = await publicClient.getCode({ address: owner.address });
const onboard = assertOnboardAllowed(code ?? "0x");
if (!onboard.ok) {
  console.error(JSON.stringify(onboard));
  process.exit(1);
}
const alreadyKernel = (code ?? "").toLowerCase().includes(KERNEL_7702_DELEGATION_ADDRESS.slice(2).toLowerCase());
if (!alreadyKernel) {
  console.error("Wallet A is not delegated to Kernel v3.3; run live:base-7702-install first.");
  process.exit(1);
}

const near = new NearService(
  new OneClickClient({
    baseUrl: process.env["ONECLICK_BASE_URL"] ?? "https://1click.chaindefuser.com",
    apiKey,
    timeoutMs: 25_000,
  }),
);

const [eth, usdc, arbUsdc] = await Promise.all([
  publicClient.getBalance({ address: owner.address }),
  publicClient.readContract({ address: USDC_BASE, abi: erc20Abi, functionName: "balanceOf", args: [owner.address] }),
  arbClient.readContract({ address: USDC_ARB, abi: erc20Abi, functionName: "balanceOf", args: [merchant as Address] }),
]);

log("balances-before", {
  owner: owner.address,
  sessionSigner: session.address,
  merchant,
  ownerEthWei: eth.toString(),
  ownerUsdc: usdc.toString(),
  merchantArbUsdc: arbUsdc.toString(),
  kernelCode: code,
});

const deadline = new Date(Date.now() + 60 * 60_000).toISOString();
let quote;
try {
  quote = await near.quote({
    dry: false,
    authorizationRef: `m4-7702-${Date.now()}`,
    originAssetId: ORIGIN,
    destinationAssetId: DEST,
    amountOut: AMOUNT_OUT.toString(),
    recipient: merchant,
    refundTo: payer,
    confidentiality: "basic",
    slippageBps: 100,
    deadline,
  });
} catch (e) {
  if (e instanceof WorkerError) {
    console.error(`quote failed ${e.code}: ${e.message}`);
    process.exit(1);
  }
  throw e;
}

const { rawResponseJson: _raw, ...redacted } = quote;
const amountIn = BigInt(quote.amountIn);
const deposit = quote.depositAddress;
if (!deposit || !isAddress(deposit)) {
  console.error("executable quote missing deposit address");
  process.exit(1);
}
if (amountIn > usdc) {
  console.error(`amountIn ${amountIn} exceeds Wallet A USDC ${usdc}`);
  process.exit(1);
}
if (amountIn > 250_000n) {
  console.error(`amountIn ${amountIn} exceeds 0.25 USDC envelope for this 0.20-out send`);
  process.exit(1);
}

log("preflight-quote", {
  ...redacted,
  permission: {
    chain: "base",
    token: USDC_BASE,
    cap: amountIn.toString(),
    recipientOnChain: "any (option C)",
    userOpRecipient: deposit,
    expirySeconds: 3600,
    rateLimitCount: 1,
    maxGasWei: MAX_GAS_WEI.toString(),
  },
});

const now = Math.floor(Date.now() / 1000);
const grant = buildGrant({
  owner: owner.address,
  sessionSigner: session.address,
  chainId: SUPPORTED_7702_CHAINS.base,
  token: USDC_BASE,
  maxAmount: amountIn,
  validUntil: now + 3600,
  nowUnix: now,
  maxGas: 2_000_000n,
  maxGasWei: MAX_GAS_WEI,
  rateLimitCount: 1,
});
const call = {
  chainId: SUPPORTED_7702_CHAINS.base,
  from: owner.address,
  to: USDC_BASE,
  data: encodeErc20Transfer(deposit, amountIn),
  value: 0n,
  callKind: "call" as const,
  gasLimit: 200_000n,
  approvalRef: APPROVAL_REF,
  attemptId: quote.correlationId,
  quoteDepositAddress: deposit,
};
const local = validateSessionCall(call, grant, now, gates, "execute");
if (!local.ok) {
  console.error(JSON.stringify(local));
  process.exit(1);
}

const fees = async () => {
  const f = await publicClient.estimateFeesPerGas();
  const minTip = 2_000_000n;
  let tip = f.maxPriorityFeePerGas ?? minTip;
  if (tip < minTip) tip = minTip;
  let max = (f.maxFeePerGas ?? tip) * 2n;
  if (max < tip) max = tip;
  try {
    const res = await fetch(BUNDLER, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "pimlico_getUserOperationGasPrice", params: [] }),
    });
    const json = (await res.json()) as {
      result?: { fast?: { maxFeePerGas: string; maxPriorityFeePerGas: string } };
    };
    const fast = json.result?.fast;
    if (fast) {
      const pTip = BigInt(fast.maxPriorityFeePerGas);
      const pMax = BigInt(fast.maxFeePerGas);
      if (pTip > tip) tip = pTip;
      if (pMax > max) max = pMax;
    }
  } catch {
    // public-client estimate is enough if the bundler gas-price method is down
  }
  return { maxFeePerGas: max, maxPriorityFeePerGas: tip };
};

const sessionMod = await toECDSASigner({ signer: session });
const permissionPlugin = await toPermissionValidator(publicClient, {
  signer: sessionMod,
  policies: buildSessionPolicies(grant, now),
  entryPoint: ENTRY_POINT,
  kernelVersion: KERNEL_VERSION,
});
const sessionKernel = await createKernelAccount(publicClient, {
  eip7702Account: owner,
  plugins: { regular: permissionPlugin },
  entryPoint: ENTRY_POINT,
  kernelVersion: KERNEL_VERSION,
});
const serialized = await serializePermissionAccount(sessionKernel);
const agentAccount = await deserializePermissionAccount(
  publicClient,
  ENTRY_POINT,
  KERNEL_VERSION,
  serialized,
  sessionMod,
);
const agentClient = createKernelAccountClient({
  account: agentAccount,
  chain: base,
  bundlerTransport: http(BUNDLER),
  client: publicClient,
  userOperation: { estimateFeesPerGas: fees },
});

log("session-transfer", { to: deposit, amount: amountIn.toString(), token: USDC_BASE });
const userOpHash = await agentClient.sendUserOperation({
  callData: await agentAccount.encodeCalls([{ to: USDC_BASE, value: 0n, data: call.data }]),
});
log("session-transfer-sent", { userOpHash });
const rcpt = await agentClient.waitForUserOperationReceipt({ hash: userOpHash });
log("session-transfer-receipt", {
  txHash: rcpt.receipt.transactionHash,
  success: rcpt.success,
});
if (!rcpt.success) {
  console.error("session UserOp failed");
  process.exit(1);
}

const txHash = rcpt.receipt.transactionHash;
try {
  await near.depositSubmitted(deposit, txHash);
  log("deposit-submit", { ok: true });
} catch (e) {
  const msg = e instanceof WorkerError ? e.message : e instanceof Error ? e.message : String(e);
  log("deposit-submit", { ok: false, error: msg });
}

const polls = Number(process.env["STATUS_POLLS"] ?? "36");
const delayMs = Number(process.env["STATUS_DELAY_MS"] ?? "5000");
let lastStatus = "";
for (let i = 0; i < polls; i += 1) {
  try {
    const s = await near.status(deposit);
    lastStatus = s.status;
    log("status", {
      poll: i,
      status: s.status,
      originTxHashes: s.originTxHashes,
      destinationTxHashes: s.destinationTxHashes,
      amountIn: s.amountIn,
      amountOut: s.amountOut,
      refundedAmount: s.refundedAmount,
    });
    if (new Set(["SUCCESS", "INCOMPLETE_DEPOSIT", "REFUNDED", "FAILED"]).has(s.status)) break;
  } catch (e) {
    const msg = e instanceof WorkerError ? `${e.code}: ${e.message}` : e instanceof Error ? e.message : String(e);
    log("status", { poll: i, error: msg });
  }
  await sleep(delayMs);
}

const [ethAfter, usdcAfter, arbAfter] = await Promise.all([
  publicClient.getBalance({ address: owner.address }),
  publicClient.readContract({ address: USDC_BASE, abi: erc20Abi, functionName: "balanceOf", args: [owner.address] }),
  arbClient.readContract({ address: USDC_ARB, abi: erc20Abi, functionName: "balanceOf", args: [merchant as Address] }),
]);
log("balances-after", {
  ownerEthWei: ethAfter.toString(),
  ownerUsdc: usdcAfter.toString(),
  merchantArbUsdc: arbAfter.toString(),
  lastStatus,
});

if (lastStatus !== "SUCCESS") {
  process.exit(2);
}
