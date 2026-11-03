/**
 * Test-only Ed25519 signer that produces quotes verifiable by the OFFICIAL
 * `verifyQuoteSignature()` when given our test public key. We hash with the
 * SDK's exported `quoteHash()` so the signed payload is exactly what 1Click
 * signs; only the key differs. This lets us test the full pipeline
 * (request -> echo -> signature -> projection) offline.
 */
import { generateKeyPairSync, sign as edSign, type KeyObject } from "node:crypto";
import { quoteHash } from "@defuse-protocol/one-click-sdk-typescript";
import { base58 } from "@scure/base";
import type { QuoteRequest, QuoteResponse } from "../../src/near/types.js";

export interface TestSigner {
  publicKey: string; // "ed25519:<base58>"
  /** `response` is a QuoteResponse without (or with a placeholder) signature. */
  sign(response: object): QuoteResponse;
}

export function makeTestSigner(): TestSigner {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const rawPub = spkiToRaw(publicKey);
  return {
    publicKey: `ed25519:${base58.encode(rawPub)}`,
    sign(response) {
      const withPlaceholder = { ...response, signature: "" } as unknown as QuoteResponse;
      const msg = Buffer.from(quoteHash(withPlaceholder as never), "utf8");
      const sig = edSign(null, msg, privateKey);
      return { ...withPlaceholder, signature: `ed25519:${base58.encode(new Uint8Array(sig))}` };
    },
  };
}

function spkiToRaw(key: KeyObject): Uint8Array {
  const der = key.export({ type: "spki", format: "der" }) as Buffer;
  // Ed25519 SPKI DER is 44 bytes: 12-byte header + 32-byte raw key.
  return new Uint8Array(der.subarray(der.length - 32));
}

let counter = 0;

/** Build a plausible 1Click response echoing `req`, signed by `signer`. */
export function fakeQuoteResponse(
  signer: TestSigner,
  req: QuoteRequest,
  overrides: { quote?: Partial<QuoteResponse["quote"]>; echo?: Partial<QuoteRequest>; timestamp?: string } = {},
): QuoteResponse {
  counter += 1;
  const amountOut = req.amount;
  const minIn = (BigInt(amountOut) * 1_002n) / 1_000n; // ~0.2% fee
  const amountIn = minIn + (minIn * BigInt(req.slippageTolerance)) / 10_000n;
  const base = {
    correlationId: `test-${counter}`,
    timestamp: overrides.timestamp ?? new Date().toISOString(),
    quoteRequest: { ...req, ...(overrides.echo ?? {}) },
    quote: {
      amountIn: amountIn.toString(),
      amountInFormatted: "x",
      amountInUsd: "x",
      minAmountIn: minIn.toString(),
      amountOut,
      amountOutFormatted: "x",
      amountOutUsd: "x",
      minAmountOut: amountOut,
      timeEstimate: 120,
      refundFee: "1000",
      withdrawFee: "1000",
      ...(req.dry
        ? {}
        : {
            depositAddress: `0x${(0xde905170 + counter).toString(16).padStart(40, "0")}`,
            deadline: req.deadline,
            timeWhenInactive: req.deadline,
          }),
      ...(overrides.quote ?? {}),
    },
  };
  return signer.sign(base);
}
