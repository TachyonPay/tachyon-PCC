/**
 * M4 live: install Kernel v3.3 EIP-7702 on Wallet A (Base mainnet).
 * Does NOT grant a session, does NOT call 1Click, does NOT transfer USDC.
 *
 * Owner key: secrets/wallet-a.key. Session key is printed only (no funds
 * required on it). Never prints private keys.
 *
 * Requires this process: DRY_RUN=false ENABLE_MAINNET_TRANSFERS=true
 * and an explicit owner approval of this Kernel on this EOA.
 */
import { resolve } from "node:path";
import { createPublicClient, erc20Abi, http } from "viem";
import { base } from "viem/chains";
import { createKernelAccount, createKernelAccountClient } from "@zerodev/sdk";
import { loadRepoEnv } from "./_env.js";
import { loadSessionSigner } from "../src/account-abstraction/keys.js";
import {
  ENTRY_POINT,
  KERNEL_VERSION,
  KERNEL_7702_DELEGATION_ADDRESS,
  SUPPORTED_7702_CHAINS,
  USDC_BASE,
} from "../src/account-abstraction/pins.js";
import { assertOnboardAllowed } from "../src/account-abstraction/validate.js";
import { gatesFromEnv, checkBroadcast } from "../src/account-abstraction/gates.js";

loadRepoEnv();

const OWNER_PATH =
  process.env["SOURCE_EXECUTOR_KEY_REF"]?.trim() || resolve(process.cwd(), "../../secrets/wallet-a.key");
const SESSION_PATH =
  process.env["SESSION_SIGNER_KEY_REF"]?.trim() || resolve(process.cwd(), "../../secrets/session-userop.key");
const RPC = process.env["RPC_URL_BASE"]?.trim() || "https://mainnet.base.org";
const BUNDLER = process.env["BUNDLER_RPC_BASE"]?.trim() || "https://public.pimlico.io/v2/8453/rpc";

function log(phase: string, data: Record<string, unknown>): void {
  console.log(JSON.stringify({ phase, ...data }));
}

const gates = gatesFromEnv();
const gate = checkBroadcast(gates, SUPPORTED_7702_CHAINS.base, "chat-base-7702-install");
if (!gate.ok) {
  console.error(
    `refusing: ${gate.reason}. Re-run with DRY_RUN=false ENABLE_MAINNET_TRANSFERS=true on this process only.`,
  );
  process.exit(2);
}

const owner = loadSessionSigner(OWNER_PATH);
const session = loadSessionSigner(SESSION_PATH);
if (owner.address.toLowerCase() === session.address.toLowerCase()) {
  console.error("owner and session signer must be different keys.");
  process.exit(2);
}

const publicClient = createPublicClient({ chain: base, transport: http(RPC) });
const code = await publicClient.getCode({ address: owner.address });
const onboard = assertOnboardAllowed(code ?? "0x");
if (!onboard.ok) {
  console.error(JSON.stringify(onboard));
  process.exit(1);
}

const [eth, usdc] = await Promise.all([
  publicClient.getBalance({ address: owner.address }),
  publicClient.readContract({
    address: USDC_BASE,
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [owner.address],
  }),
]);

log("preflight", {
  chain: "base",
  chainId: SUPPORTED_7702_CHAINS.base,
  kernelImplementation: KERNEL_7702_DELEGATION_ADDRESS,
  kernelVersion: KERNEL_VERSION,
  owner: owner.address,
  sessionSigner: session.address,
  sessionSignerNeedsFunds: false,
  ownerEthWei: eth.toString(),
  ownerUsdc: usdc.toString(),
  onboard,
  bundler: BUNDLER,
  permission: {
    token: USDC_BASE,
    note: "not granted in this script; install only",
  },
});

if (eth === 0n) {
  console.error("owner needs Base ETH for the 7702 install UserOp.");
  process.exit(1);
}

if (onboard.ok && (code ?? "0x") !== "0x" && (code ?? "").toLowerCase().includes(KERNEL_7702_DELEGATION_ADDRESS.slice(2).toLowerCase())) {
  log("already-installed", { code, stop: true });
  process.exit(0);
}

const fees = async () => {
  const f = await publicClient.estimateFeesPerGas();
  const tip = f.maxPriorityFeePerGas ?? 1_000_000n;
  const max = (f.maxFeePerGas ?? tip) * 2n;
  return { maxFeePerGas: max < 1_000_000n ? 1_000_000n : max, maxPriorityFeePerGas: tip };
};

const ownerAccount = await createKernelAccount(publicClient, {
  eip7702Account: owner,
  entryPoint: ENTRY_POINT,
  kernelVersion: KERNEL_VERSION,
});
const ownerClient = createKernelAccountClient({
  account: ownerAccount,
  chain: base,
  bundlerTransport: http(BUNDLER),
  client: publicClient,
  userOperation: { estimateFeesPerGas: fees },
});

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
log("stop", {
  next: "session grant + 1Click deposit need a separate owner approval of one previewed UserOp",
});
