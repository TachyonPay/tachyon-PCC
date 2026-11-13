/**
 * M3 live spike on Base Sepolia (84532). 1Click is not involved.
 *
 * Owner is the funded EOA (session-signer.key / 0xF5273…). A *new* session
 * key signs UserOps. Bundler: Pimlico public (no ZeroDev project key).
 *
 * Never prints private keys. DRY_RUN=false must be set on this process.
 */
import { existsSync, mkdirSync, writeFileSync, chmodSync } from "node:fs";
import { resolve } from "node:path";
import {
  createPublicClient,
  encodeFunctionData,
  erc20Abi,
  http,
  isAddress,
  type Address,
} from "viem";
import { baseSepolia } from "viem/chains";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";
import { createKernelAccount, createKernelAccountClient } from "@zerodev/sdk";
import { toPermissionValidator } from "@zerodev/permissions";
import { toECDSASigner } from "@zerodev/permissions/signers";
import { serializePermissionAccount, deserializePermissionAccount } from "@zerodev/permissions";
import { loadRepoEnv } from "./_env.js";
import { loadSessionSigner } from "../src/account-abstraction/keys.js";
import {
  ENTRY_POINT,
  KERNEL_VERSION,
  KERNEL_7702_DELEGATION_ADDRESS,
  SUPPORTED_7702_CHAINS,
  USDC_BASE_SEPOLIA,
} from "../src/account-abstraction/pins.js";
import { assertOnboardAllowed } from "../src/account-abstraction/validate.js";
import { buildGrant, buildSessionPolicies } from "../src/account-abstraction/grant.js";
import { encodeErc20Transfer } from "../src/account-abstraction/call.js";
import { gatesFromEnv, checkBroadcast } from "../src/account-abstraction/gates.js";

loadRepoEnv();

const OWNER_PATH =
  process.env["SESSION_SIGNER_KEY_REF"]?.trim() || resolve(process.cwd(), "../../secrets/session-signer.key");
const SESSION_PATH = resolve(process.cwd(), "../../secrets/session-userop.key");
const RPC = process.env["RPC_URL_BASE_SEPOLIA"]?.trim() || "https://sepolia.base.org";
const BUNDLER = process.env["BUNDLER_RPC_BASE_SEPOLIA"]?.trim() || "https://public.pimlico.io/v2/84532/rpc";
const RECIPIENT = (process.env["WALLET_A_ADDRESS"] ?? "0x049FdEf2550772B9F36F86757C27f5F982D0970D").trim();
const AMOUNT = 1_000_000n; // 1 USDC
const CAP = 2_000_000n; // 2 USDC

if (!isAddress(RECIPIENT)) {
  console.error("WALLET_A_ADDRESS must be an EVM address.");
  process.exit(2);
}

function ensureSessionUserOpKey(): ReturnType<typeof privateKeyToAccount> {
  if (existsSync(SESSION_PATH)) return loadSessionSigner(SESSION_PATH);
  mkdirSync(resolve(SESSION_PATH, ".."), { recursive: true });
  const pk = generatePrivateKey();
  writeFileSync(SESSION_PATH, pk + "\n", { mode: 0o600 });
  chmodSync(SESSION_PATH, 0o600);
  return privateKeyToAccount(pk);
}

function log(phase: string, data: Record<string, unknown>): void {
  console.log(JSON.stringify({ phase, ...data }));
}

const gates = gatesFromEnv();
const gate = checkBroadcast(gates, SUPPORTED_7702_CHAINS.baseSepolia, "chat-base-sepolia-7702");
if (!gate.ok) {
  console.error(`refusing: ${gate.reason}. Re-run with DRY_RUN=false on this process only.`);
  process.exit(2);
}

const owner = loadSessionSigner(OWNER_PATH);
const session = ensureSessionUserOpKey();
if (owner.address.toLowerCase() === session.address.toLowerCase()) {
  console.error("owner and session signer must be different keys.");
  process.exit(2);
}

const publicClient = createPublicClient({ chain: baseSepolia, transport: http(RPC) });
const code = await publicClient.getCode({ address: owner.address });
const onboard = assertOnboardAllowed(code ?? "0x");
if (!onboard.ok) {
  console.error(JSON.stringify(onboard));
  process.exit(1);
}

const [eth, usdc] = await Promise.all([
  publicClient.getBalance({ address: owner.address }),
  publicClient.readContract({
    address: USDC_BASE_SEPOLIA,
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [owner.address],
  }),
]);

log("preflight", {
  chain: "base-sepolia",
  chainId: SUPPORTED_7702_CHAINS.baseSepolia,
  kernelImplementation: KERNEL_7702_DELEGATION_ADDRESS,
  kernelVersion: KERNEL_VERSION,
  owner: owner.address,
  sessionSigner: session.address,
  recipient: RECIPIENT,
  ownerEthWei: eth.toString(),
  ownerUsdc: usdc.toString(),
  onboard,
  amount: AMOUNT.toString(),
  cap: CAP.toString(),
  bundler: BUNDLER,
});

if (eth === 0n || usdc < AMOUNT) {
  console.error("owner needs Base Sepolia ETH and at least 1 USDC.");
  process.exit(1);
}

const now = Math.floor(Date.now() / 1000);
const grant = buildGrant({
  owner: owner.address,
  sessionSigner: session.address,
  chainId: SUPPORTED_7702_CHAINS.baseSepolia,
  token: USDC_BASE_SEPOLIA,
  maxAmount: CAP,
  validUntil: now + 3600,
  nowUnix: now,
  maxGas: 2_000_000n,
  maxGasWei: 50_000_000_000_000_000n, // 0.05 ETH
  rateLimitCount: 5,
});
const policies = buildSessionPolicies(grant, now);

const fees = async () => {
  const f = await publicClient.estimateFeesPerGas();
  const tip = f.maxPriorityFeePerGas ?? 1_000_000n;
  const max = (f.maxFeePerGas ?? tip) * 2n;
  return { maxFeePerGas: max < 1_000_000n ? 1_000_000n : max, maxPriorityFeePerGas: tip };
};

async function kernelClient(account: Awaited<ReturnType<typeof createKernelAccount>>) {
  return createKernelAccountClient({
    account,
    chain: baseSepolia,
    bundlerTransport: http(BUNDLER),
    client: publicClient,
    userOperation: { estimateFeesPerGas: fees },
  });
}

// 1) Owner sudo: install 7702 designator with a no-op call.
const ownerAccount = await createKernelAccount(publicClient, {
  eip7702Account: owner,
  entryPoint: ENTRY_POINT,
  kernelVersion: KERNEL_VERSION,
});
const ownerClient = await kernelClient(ownerAccount);
log("install-7702", { from: owner.address, to: KERNEL_7702_DELEGATION_ADDRESS });
const installHash = await ownerClient.sendUserOperation({
  callData: await ownerAccount.encodeCalls([{ to: owner.address, value: 0n, data: "0x" }]),
});
log("install-7702-sent", { userOpHash: installHash });
const installRcpt = await ownerClient.waitForUserOperationReceipt({ hash: installHash });
const codeAfter = await publicClient.getCode({ address: owner.address });
log("install-7702-receipt", {
  txHash: installRcpt.receipt.transactionHash,
  success: installRcpt.success,
  code: codeAfter,
});
if (!installRcpt.success) {
  console.error("7702 install UserOp failed");
  process.exit(1);
}

// 2) Serialize a session-key approval (owner signs enable once), then the
//    session key alone sends the transfer — no second owner signature.
const sessionMod = await toECDSASigner({ signer: session });
const permissionPlugin = await toPermissionValidator(publicClient, {
  signer: sessionMod,
  policies,
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
const agentClient = await kernelClient(agentAccount);

const transferData = encodeErc20Transfer(RECIPIENT as Address, AMOUNT);
const transferHash = await agentClient.sendUserOperation({
  callData: await agentAccount.encodeCalls([
    { to: USDC_BASE_SEPOLIA, value: 0n, data: transferData },
  ]),
});
log("session-transfer-sent", { userOpHash: transferHash, to: RECIPIENT, amount: AMOUNT.toString() });
const transferRcpt = await agentClient.waitForUserOperationReceipt({ hash: transferHash });
log("session-transfer-receipt", {
  txHash: transferRcpt.receipt.transactionHash,
  success: transferRcpt.success,
});

// 3) Out-of-policy: approve must fail (session key, no owner).
const approveData = encodeFunctionData({
  abi: erc20Abi,
  functionName: "approve",
  args: [RECIPIENT as Address, AMOUNT],
});
let negative: { rejected: boolean; error?: string } = { rejected: false };
try {
  await agentClient.sendUserOperation({
    callData: await agentAccount.encodeCalls([{ to: USDC_BASE_SEPOLIA, value: 0n, data: approveData }]),
  });
} catch (e) {
  negative = { rejected: true, error: e instanceof Error ? e.message.slice(0, 200) : String(e) };
}
log("negative-approve", negative);

const [ethAfter, usdcOwner, usdcRecipient] = await Promise.all([
  publicClient.getBalance({ address: owner.address }),
  publicClient.readContract({
    address: USDC_BASE_SEPOLIA,
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [owner.address],
  }),
  publicClient.readContract({
    address: USDC_BASE_SEPOLIA,
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [RECIPIENT as Address],
  }),
]);
log("balances-after", {
  ownerEthWei: ethAfter.toString(),
  ownerUsdc: usdcOwner.toString(),
  recipientUsdc: usdcRecipient.toString(),
});

if (!transferRcpt.success || !negative.rejected) {
  process.exit(1);
}
