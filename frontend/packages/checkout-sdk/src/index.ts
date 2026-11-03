import type { GatesView, Health } from "@pcc/api-types";

export interface CheckoutClientOptions {
  baseUrl: string;
  fetchImpl?: typeof fetch;
  timeoutMs?: number;
}

export class CheckoutApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "CheckoutApiError";
  }
}

/**
 * Minimal typed client. Every method is a plain fetch against the Rust API;
 * no wallet access, no signing, no secrets. Payment-intent methods arrive
 * with the M6 API.
 */
export class CheckoutClient {
  private readonly fetchImpl: typeof fetch;
  private readonly timeoutMs: number;
  private readonly baseUrl: string;

  constructor(opts: CheckoutClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.fetchImpl = opts.fetchImpl ?? fetch;
    this.timeoutMs = opts.timeoutMs ?? 10_000;
  }

  health(): Promise<Health> {
    return this.get<Health>("/health");
  }

  gates(): Promise<GatesView> {
    return this.get<GatesView>("/v1/meta/gates");
  }

  private async get<T>(path: string): Promise<T> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      const res = await this.fetchImpl(`${this.baseUrl}${path}`, { signal: controller.signal, headers: { Accept: "application/json" } });
      if (!res.ok) throw new CheckoutApiError(res.status, `GET ${path} failed with ${res.status}`);
      return (await res.json()) as T;
    } finally {
      clearTimeout(timer);
    }
  }
}
