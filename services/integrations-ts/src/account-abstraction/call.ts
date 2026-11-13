/**
 * ERC-20 transfer calldata for a session-key UserOp. Approve and native value
 * are refused here, before anything is signed.
 */
import { decodeFunctionData, encodeFunctionData, erc20Abi, isAddress, type Address, type Hex } from "viem";

export const TRANSFER_SELECTOR = "0xa9059cbb";
export const APPROVE_SELECTOR = "0x095ea7b3";

export class CallError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CallError";
  }
}

export function selectorOf(data: Hex): string {
  if (!data || data === "0x" || data.length < 10) throw new CallError("calldata too short");
  return data.slice(0, 10).toLowerCase();
}

export function encodeErc20Transfer(to: Address, amount: bigint): Hex {
  if (!isAddress(to)) throw new CallError("recipient must be an address");
  if (amount <= 0n) throw new CallError("amount must be > 0");
  return encodeFunctionData({ abi: erc20Abi, functionName: "transfer", args: [to, amount] });
}

export function decodeErc20Transfer(data: Hex): { to: Address; amount: bigint } {
  const sel = selectorOf(data);
  if (sel === APPROVE_SELECTOR) throw new CallError("refusing ERC-20 approve");
  if (sel !== TRANSFER_SELECTOR) throw new CallError(`not an erc20 transfer: selector ${sel}`);
  const decoded = decodeFunctionData({ abi: erc20Abi, data });
  if (decoded.functionName !== "transfer") throw new CallError("not an erc20 transfer");
  const [to, amount] = decoded.args;
  return { to, amount };
}
