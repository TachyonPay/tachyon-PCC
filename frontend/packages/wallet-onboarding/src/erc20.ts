/**
 * ERC-20 `transfer(address,uint256)` calldata. Used when MetaMask funds the
 * Privy checkout wallet. Never encodes `approve`.
 */
import { encodeFunctionData, erc20Abi, isAddress, type Address, type Hex } from "viem";

export const TRANSFER_SELECTOR = "0xa9059cbb";

export function encodeErc20Transfer(to: Address, amount: bigint): Hex {
  if (!isAddress(to)) throw new Error("recipient must be an address");
  if (amount <= 0n) throw new Error("amount must be greater than 0");
  return encodeFunctionData({ abi: erc20Abi, functionName: "transfer", args: [to, amount] });
}
