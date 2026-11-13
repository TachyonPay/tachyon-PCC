/**
 * Gated live Kernel bundler. Each UserOp runs under the payer's enabled
 * allowance for that token (see session_grants.ts); no owner key is held here. Sign
 * submits the UserOp; send waits for the receipt. Gates in
 * SessionKeyExecutor still refuse unless DRY_RUN is off. Never logs keys or
 * RPC URLs.
 */
import { decodeFunctionData, erc20Abi, http, type Hex } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { createKernelAccountClient } from "@zerodev/sdk";
import { deserializePermissionAccount } from "@zerodev/permissions";
import { toECDSASigner } from "@zerodev/permissions/signers";
import type { BundlerPort, BundlerReceipt, BundlerSimulation } from "./executor.js";
import { bundlerUrlsFromEnv, chainOf, publicClientFor, requireKernelDelegation, rpcUrlsFromEnv } from "./kernel_chain.js";
import { CHECKOUT_MAX_GAS_WEI, ENTRY_POINT, KERNEL_VERSION } from "./pins.js";
import type { SessionGrantService } from "./session_grants.js";
import type { SessionCall } from "./validate.js";

const MAX_GAS_WEI = CHECKOUT_MAX_GAS_WEI;

export class KernelBundler implements BundlerPort {
  private readonly receipts = new Map<Hex, BundlerReceipt>();
  private readonly pending = new Map<Hex, { wait: () => Promise<BundlerReceipt> }>();

  constructor(
    private readonly grants: SessionGrantService,
    private readonly rpcUrls: Record<number, string>,
    private readonly bundlerUrls: Record<number, string>,
  ) {}

  static fromEnv(grants: SessionGrantService): KernelBundler {
    return new KernelBundler(grants, rpcUrlsFromEnv(), bundlerUrlsFromEnv());
  }

  private bundlerUrlFor(chainId: number): string {
    const url = this.bundlerUrls[chainId];
    if (!url) throw new Error(`unsupported_chain: no bundler for ${chainId}`);
    return url;
  }

  receipt(userOpHash: Hex): BundlerReceipt | undefined {
    return this.receipts.get(userOpHash);
  }

  /** Balance checks plus bundler gas estimation for the exact UserOp. Never submits. */
  async simulate(call: SessionCall): Promise<BundlerSimulation> {
    let amount: bigint;
    try {
      amount = this.transferAmount(call);
    } catch (e) {
      return { ok: false, gasEstimate: 0n, notes: [e instanceof Error ? e.message : String(e)] };
    }
    const { publicClient, agentAccount, agentClient } = await this.agent(call);
    const [tokenBalance, ethBalance] = await Promise.all([
      publicClient.readContract({ address: call.to, abi: erc20Abi, functionName: "balanceOf", args: [call.from] }),
      publicClient.getBalance({ address: call.from }),
    ]);
    const notes: string[] = [];
    let ok = true;
    if (tokenBalance < amount) {
      ok = false;
      notes.push(`insufficient token balance: have ${tokenBalance}, need ${amount}`);
    }
    const op = await agentClient.prepareUserOperation({
      callData: await agentAccount.encodeCalls([{ to: call.to, value: 0n, data: call.data }]),
    });
    const gas = op.callGasLimit + op.verificationGasLimit + op.preVerificationGas;
    const maxGasCost = gas * op.maxFeePerGas;
    if (maxGasCost > MAX_GAS_WEI) {
      ok = false;
      notes.push(`gas cost ceiling ${maxGasCost} wei exceeds session cap ${MAX_GAS_WEI}`);
    }
    if (ethBalance < maxGasCost) {
      ok = false;
      notes.push(`insufficient ETH for gas: have ${ethBalance} wei, worst case ${maxGasCost} wei`);
    }
    notes.push(`bundler estimate gas=${gas} maxGasCostWei=${maxGasCost} ethBalanceWei=${ethBalance}`);
    return { ok, gasEstimate: gas, notes, senderBalance: tokenBalance };
  }

  async sign(call: SessionCall): Promise<Hex> {
    this.transferAmount(call);
    const { agentAccount, agentClient } = await this.agent(call);
    const userOpHash = await agentClient.sendUserOperation({
      callData: await agentAccount.encodeCalls([{ to: call.to, value: 0n, data: call.data }]),
    });
    this.pending.set(userOpHash, {
      wait: async () => {
        const rcpt = await agentClient.waitForUserOperationReceipt({ hash: userOpHash });
        const rec: BundlerReceipt = {
          txHash: rcpt.receipt.transactionHash,
          blockNumber: rcpt.receipt.blockNumber ?? 0n,
          success: rcpt.success,
          gasUsed: rcpt.receipt.gasUsed ?? 0n,
        };
        this.receipts.set(userOpHash, rec);
        return rec;
      },
    });
    return userOpHash;
  }

  private transferAmount(call: SessionCall): bigint {
    const decoded = decodeFunctionData({ abi: erc20Abi, data: call.data });
    if (decoded.functionName !== "transfer") throw new Error("malicious_selector");
    return decoded.args[1] as bigint;
  }

  private async agent(call: SessionCall) {
    const active = this.grants.activeFor(call.chainId, call.from, call.to, call.mandateId);
    if (!active) throw Object.assign(new Error("no_session: payer has no active allowance for this token"), { code: "no_session" });
    const chain = chainOf(call.chainId);
    const publicClient = publicClientFor(this.rpcUrls, call.chainId);
    await requireKernelDelegation(publicClient, call.from, call.chainId);
    const sessionMod = await toECDSASigner({ signer: privateKeyToAccount(active.sessionKey) });
    const agentAccount = await deserializePermissionAccount(
      publicClient,
      ENTRY_POINT,
      KERNEL_VERSION,
      active.serialized,
      sessionMod,
    );
    const bundlerUrl = this.bundlerUrlFor(call.chainId);
    const agentClient = createKernelAccountClient({
      account: agentAccount,
      chain,
      bundlerTransport: http(bundlerUrl),
      client: publicClient,
      userOperation: {
        estimateFeesPerGas: async () => fees(publicClient, bundlerUrl),
      },
    });
    return { publicClient, agentAccount, agentClient };
  }

  async send(userOpHash: Hex): Promise<void> {
    const pending = this.pending.get(userOpHash);
    if (pending) {
      await pending.wait();
      this.pending.delete(userOpHash);
      return;
    }
    if (!this.receipts.has(userOpHash)) {
      throw new Error("unknown userOp hash; nothing to wait for");
    }
  }
}

async function fees(
  publicClient: { estimateFeesPerGas: () => Promise<{ maxFeePerGas?: bigint; maxPriorityFeePerGas?: bigint }> },
  bundlerUrl: string,
): Promise<{ maxFeePerGas: bigint; maxPriorityFeePerGas: bigint }> {
  const f = await publicClient.estimateFeesPerGas();
  const minTip = 2_000_000n;
  let tip = f.maxPriorityFeePerGas ?? minTip;
  if (tip < minTip) tip = minTip;
  let max = (f.maxFeePerGas ?? tip) * 2n;
  if (max < tip) max = tip;
  try {
    const res = await fetch(bundlerUrl, {
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
}
