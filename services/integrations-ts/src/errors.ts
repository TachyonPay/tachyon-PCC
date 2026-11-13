/**
 * Error codes shared with the Rust contract (`ErrorCode` in
 * backend/crates/integration_client/src/contract.rs). Keep in sync; the
 * contract test checks the OpenAPI enum against this list.
 */
export const ERROR_CODES = [
  "UNAUTHORIZED",
  "INVALID_REQUEST",
  "AUTH_REF_REUSED",
  "UPSTREAM_TIMEOUT",
  "UPSTREAM_UNAUTHORIZED",
  "UPSTREAM_RATE_LIMITED",
  "UPSTREAM_REJECTED",
  "UPSTREAM_UNAVAILABLE",
  "MALFORMED_UPSTREAM",
  "SIGNATURE_INVALID",
  "NO_ROUTE",
  "SESSION_REJECTED",
  "INTERNAL",
] as const;
export type ErrorCode = (typeof ERROR_CODES)[number];

export class WorkerError extends Error {
  constructor(
    public readonly code: ErrorCode,
    message: string,
    public readonly httpStatus: number,
    public readonly detail?: unknown,
  ) {
    super(message);
    this.name = "WorkerError";
  }

  toBody(): { error: { code: ErrorCode; message: string } } {
    return { error: { code: this.code, message: this.message } };
  }
}

export const err = {
  unauthorized: (m = "missing or invalid internal token") => new WorkerError("UNAUTHORIZED", m, 401),
  invalid: (m: string) => new WorkerError("INVALID_REQUEST", m, 400),
  authRefReused: (ref: string) => new WorkerError("AUTH_REF_REUSED", `authorizationRef already used: ${ref}`, 409),
  upstreamTimeout: () => new WorkerError("UPSTREAM_TIMEOUT", "1Click request timed out", 504),
  upstreamUnauthorized: () => new WorkerError("UPSTREAM_UNAUTHORIZED", "1Click rejected API key", 401),
  upstreamRateLimited: () => new WorkerError("UPSTREAM_RATE_LIMITED", "1Click rate limit", 429),
  upstreamRejected: (m: string) => new WorkerError("UPSTREAM_REJECTED", m, 422),
  upstreamUnavailable: (m: string) => new WorkerError("UPSTREAM_UNAVAILABLE", m, 502),
  malformedUpstream: (m: string, cause?: unknown) => new WorkerError("MALFORMED_UPSTREAM", m, 502, cause),
  signatureInvalid: (m = "quote signature verification failed") => new WorkerError("SIGNATURE_INVALID", m, 422),
  noRoute: (m: string) => new WorkerError("NO_ROUTE", m, 422),
  sessionRejected: (m: string) => new WorkerError("SESSION_REJECTED", m, 422),
  internal: (m = "internal error") => new WorkerError("INTERNAL", m, 500),
};
