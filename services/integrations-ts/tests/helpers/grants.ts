import type { Hex, TypedDataDefinition } from "viem";
import type { GrantBackend, GrantSpec } from "../../src/account-abstraction/session_grants.js";

/**
 * Chain-free grant backend: a real EIP-712 payload (so signature recovery is
 * exercised for real) and a serialization that records the signature.
 */
export class FakeGrantBackend implements GrantBackend {
  delegated = true;
  readonly serialized: { spec: GrantSpec; signature: Hex }[] = [];

  async enableTypedData(spec: GrantSpec): Promise<TypedDataDefinition> {
    if (!this.delegated) {
      throw Object.assign(new Error(`not_delegated: owner has no Kernel 7702 delegation on chain ${spec.grant.chainId}`), {
        code: "not_delegated",
      });
    }
    return {
      domain: { name: "Kernel", version: "0.3.3", chainId: spec.grant.chainId, verifyingContract: spec.grant.owner },
      types: {
        Enable: [
          { name: "sessionSigner", type: "address" },
          { name: "token", type: "address" },
          { name: "maxAmount", type: "uint256" },
          { name: "uses", type: "uint48" },
          { name: "gasBudget", type: "uint256" },
          { name: "validUntil", type: "uint48" },
        ],
      },
      primaryType: "Enable",
      message: {
        sessionSigner: spec.grant.sessionSigner,
        token: spec.grant.token,
        maxAmount: spec.grant.maxAmount,
        uses: spec.grant.rateLimitCount,
        gasBudget: spec.grant.maxGasWei,
        validUntil: spec.grant.validUntil,
      },
    } as TypedDataDefinition;
  }

  async serialize(spec: GrantSpec, signature: Hex): Promise<string> {
    this.serialized.push({ spec, signature });
    return `fake:${signature}`;
  }
}
