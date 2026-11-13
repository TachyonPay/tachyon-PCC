/**
 * ZeroDev implementation of the grant backend. The owner is a watch-only
 * account: any attempt by the SDK to sign with it throws, so the only owner
 * signature used is the one the payer's wallet produced over the typed data.
 */
import { type Address, type Hex, type TypedDataDefinition } from "viem";
import { privateKeyToAccount, toAccount } from "viem/accounts";
import { createKernelAccount } from "@zerodev/sdk";
import { serializePermissionAccount, toPermissionValidator } from "@zerodev/permissions";
import { toECDSASigner } from "@zerodev/permissions/signers";
import { buildSessionPolicies } from "./grant.js";
import { publicClientFor, requireKernelDelegation, rpcUrlsFromEnv } from "./kernel_chain.js";
import { ENTRY_POINT, KERNEL_VERSION } from "./pins.js";
import type { GrantBackend, GrantSpec } from "./session_grants.js";

export function watchOnly(address: Address) {
  const refuse = async (): Promise<never> => {
    throw new Error("watch-only owner: the payer signs in their own wallet");
  };
  return toAccount({ address, signMessage: refuse, signTransaction: refuse, signTypedData: refuse });
}

export class KernelGrantBackend implements GrantBackend {
  constructor(private readonly rpcUrls: Record<number, string>) {}

  static fromEnv(): KernelGrantBackend {
    return new KernelGrantBackend(rpcUrlsFromEnv());
  }

  private async sessionKernel(spec: GrantSpec) {
    const { grant } = spec;
    const publicClient = publicClientFor(this.rpcUrls, grant.chainId);
    await requireKernelDelegation(publicClient, grant.owner, grant.chainId);
    const sessionMod = await toECDSASigner({ signer: privateKeyToAccount(spec.sessionKey) });
    const permissionPlugin = await toPermissionValidator(publicClient, {
      signer: sessionMod,
      policies: buildSessionPolicies(grant, spec.nowUnix),
      entryPoint: ENTRY_POINT,
      kernelVersion: KERNEL_VERSION,
    });
    return createKernelAccount(publicClient, {
      eip7702Account: watchOnly(grant.owner),
      plugins: { regular: permissionPlugin },
      entryPoint: ENTRY_POINT,
      kernelVersion: KERNEL_VERSION,
    });
  }

  async enableTypedData(spec: GrantSpec): Promise<TypedDataDefinition> {
    const kernel = await this.sessionKernel(spec);
    return (await kernel.kernelPluginManager.getPluginsEnableTypedData(kernel.address)) as TypedDataDefinition;
  }

  async serialize(spec: GrantSpec, enableSignature: Hex): Promise<string> {
    const kernel = await this.sessionKernel(spec);
    return serializePermissionAccount(kernel, undefined, enableSignature);
  }
}
