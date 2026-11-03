import type { Address, PublicClient } from "viem";
import { parseDelegation, type DelegationState } from "./delegation.js";

/** Read-only: fetches account code via the provided client and parses it. */
export async function readDelegation(client: Pick<PublicClient, "getCode">, address: Address): Promise<DelegationState> {
  const code = await client.getCode({ address });
  return parseDelegation(code);
}
