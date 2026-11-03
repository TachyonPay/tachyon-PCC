/**
 * Quote verification = official Ed25519 signature check + request-binding
 * checks for fields the signature does NOT cover.
 *
 * Verified against `@defuse-protocol/one-click-sdk-typescript` 0.1.26
 * (installed; dist/index.mjs `buildSignedQuoteRequest`/`buildSignedQuote`/
 * `hashQuote`): the signed payload is `stableStringify({...req, ...quote,
 * timestamp})` where `req` is a fixed subset of `quoteRequest` fields and
 * `quote` is the amounts (plus depositAddress/memo/deadline/timeWhenInactive/
 * timeEstimate/refundFee/withdrawFee for non-dry quotes).
 *
 * NOT covered by the signature (as of 0.1.26): `confidentiality`,
 * `depositMode`, `appFees`, `sessionId`, `connectedWallets`, `rebates`,
 * `insured`. Additionally, because of the object spread, on NON-dry quotes
 * `quote.deadline` shadows `quoteRequest.deadline`, so the echoed request
 * deadline is not covered either (it is on dry quotes). Both facts are
 * pinned by tests in tests/near/verify.test.ts.
 * Therefore we additionally require that the echoed request
 * matches what we sent for those fields. This protects against a buggy or
 * downgraded upstream echo, but a MITM that can forge the echo could still
 * flip `confidentiality` in the echoed request. Mitigation: TLS to 1Click
 * (we refuse non-https base URLs) and the Rust layer re-checking the field.
 * This limitation is recorded in docs/sdk-compatibility.md.
 */
import { verifyQuoteSignature } from "@defuse-protocol/one-click-sdk-typescript";
import { err } from "../errors.js";
import type { QuoteRequest, QuoteResponse } from "./types.js";

export const ONE_CLICK_PRODUCTION_MANAGER_PUB_KEY = "ed25519:reYaWhvwu8Jzo3WUM3zhn6VrhuMEF4eADL17qtRVifc";
/** Only for fixture tests; never used at runtime. */
export const ONE_CLICK_STAGING_MANAGER_PUB_KEY = "ed25519:5J5tkaxyPoR3Q9S8LXfo5bWnXK5Z2bctJ4mB9gENh7co";

export interface VerifyOptions {
  /** The request we actually sent. If provided, echo-binding checks run. */
  sentRequest?: QuoteRequest;
  /** Override for tests. Defaults to the production key embedded in the SDK. */
  managerPublicKey?: string;
}

/** Fields that must match between what we sent and what 1Click echoed. */
const ECHO_BOUND_FIELDS: (keyof QuoteRequest)[] = [
  "dry",
  "swapType",
  "slippageTolerance",
  "originAsset",
  "depositType",
  "destinationAsset",
  "amount",
  "refundTo",
  "refundType",
  "recipient",
  "recipientType",
  "deadline",
  "confidentiality",
  "depositMode",
];

export function verifyQuoteResponse(response: QuoteResponse, opts: VerifyOptions = {}): void {
  // Cast: the SDK's model type is structurally identical to our zod output
  // for all fields the verifier reads.
  const ok = opts.managerPublicKey
    ? verifyQuoteSignature(response as never, opts.managerPublicKey)
    : verifyQuoteSignature(response as never);
  if (!ok) throw err.signatureInvalid();

  if (opts.sentRequest) {
    const echoed = response.quoteRequest as Record<string, unknown>;
    const sent = opts.sentRequest as Record<string, unknown>;
    for (const f of ECHO_BOUND_FIELDS) {
      const s = sent[f];
      const e = echoed[f];
      // Treat null/undefined as "absent" on both sides; defaults from 1Click
      // for fields we did not send are acceptable only when we did not send them.
      if (s === undefined && (e === undefined || e === null)) continue;
      if (f === "deadline" && typeof s === "string" && typeof e === "string") {
        if (Date.parse(s) !== Date.parse(e)) throw err.signatureInvalid(`echoed deadline differs from sent deadline`);
        continue;
      }
      if (f === "depositMode" && s === undefined) continue; // 1Click defaults to SIMPLE
      if (s !== e) throw err.signatureInvalid(`echoed quoteRequest.${f} differs from sent value`);
    }
  }
}
