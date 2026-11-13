import type {
  ApiKeyResponse,
  AuthorizeResponse,
  ForgetResponse,
  SiweVerifyResponse,
  DelegationStatusResponse,
  AllowanceView,
  ChargeView,
  GatesView,
  MandateSetupRequest,
  MandateSetupView,
  MandateView,
  Health,
  PayoutRequest,
  PayoutView,
  PrepareGrantRequest,
  RevokeResponse,
  SessionGrantView,
  SignupResponse,
  WebhookSecretResponse,
} from "@pcc/api-types";

export type { ChainDelegationView } from "@pcc/api-types";
export type {
  ApiKeyResponse,
  AuthorizeResponse,
  ForgetResponse,
  SiweVerifyResponse,
  AllowanceView,
  ChargeView,
  DelegationStatusResponse,
  MandateSetupRequest,
  MandateSetupView,
  MandateView,
  PayoutRequest,
  PrepareGrantRequest,
  RevokeResponse,
  PayoutView,
  SessionGrantView,
  SignupResponse,
  WebhookSecretResponse,
};

export interface CheckoutClientOptions {
  baseUrl: string;
  /** Merchant API key. Must never be supplied from a browser / hosted checkout page. */
  apiKey?: string;
  fetchImpl?: typeof fetch;
  timeoutMs?: number;
}

export class CheckoutApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    /** Server error code (e.g. `session_grant_required`) when the body carried one. */
    public readonly code?: string,
  ) {
    super(message);
    this.name = "CheckoutApiError";
  }
}

export interface PaymentIntentView {
  id: string;
  merchant_id: string;
  merchant_order_id: string;
  status: string;
  output_amount: string;
  expires_at: string;
  checkout_session_id: string;
  checkout_url: string;
}

export interface CheckoutView {
  session_id: string;
  merchant_name: string;
  merchant_order_id: string;
  output_amount: string;
  destination_asset: string;
  source_asset: string;
  receiving_address: string;
  expires_at: string;
  attempt_id: string | null;
  quoted_input: string | null;
  max_debit: string | null;
  deposit_address: string | null;
  attempt_status: string | null;
  status: string;
  success_url: string | null;
  intent_id: string;
}

export function formatBaseUnits(base: string, decimals = 6): string {
  if (!/^[0-9]+$/.test(base)) return base;
  const padded = base.padStart(decimals + 1, "0");
  const whole = padded.slice(0, padded.length - decimals);
  const frac = padded.slice(padded.length - decimals).replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : whole;
}

/** Decimal string to base units without floating point; null when malformed. */
export function parseBaseUnits(value: string, decimals = 6): string | null {
  const m = /^([0-9]*)(?:\.([0-9]*))?$/.exec(value.trim());
  if (!m || (!m[1] && !m[2])) return null;
  const frac = m[2] ?? "";
  if (frac.length > decimals) return null;
  const digits = `${m[1] || "0"}${frac.padEnd(decimals, "0")}`.replace(/^0+(?=[0-9])/, "");
  return digits;
}

export interface SourceAssetView {
  asset_id: string;
  symbol: string;
  chain_id: number;
  decimals: number;
  token_address: string;
}

export interface CreateIntentInput {
  merchantOrderId: string;
  outputAmount: string;
  successUrl?: string;
  idempotencyKey: string;
}

interface SendOptions {
  auth?: boolean;
  idempotencyKey?: string;
  headers?: Record<string, string>;
}

/**
 * Typed client for the Rust public API. Merchant-key methods must not run in a
 * payer browser; hosted checkout uses only the public session routes.
 */
export class CheckoutClient {
  private readonly fetchImpl: typeof fetch;
  private readonly timeoutMs: number;
  private readonly baseUrl: string;
  private readonly apiKey: string | undefined;

  constructor(opts: CheckoutClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.fetchImpl = opts.fetchImpl ?? ((input, init) => fetch(input, init));
    this.timeoutMs = opts.timeoutMs ?? 25_000;
    this.apiKey = opts.apiKey;
  }

  health(): Promise<Health> {
    return this.get<Health>("/health");
  }

  gates(): Promise<GatesView> {
    return this.get<GatesView>("/v1/meta/gates");
  }

  me(): Promise<{ id: string; name: string; webhook_url: string | null }> {
    return this.get("/v1/me", { auth: true });
  }

  createIntent(input: CreateIntentInput): Promise<PaymentIntentView> {
    const body: Record<string, string> = {
      merchant_order_id: input.merchantOrderId,
      output_amount: input.outputAmount,
    };
    if (input.successUrl !== undefined) body["success_url"] = input.successUrl;
    return this.post("/v1/payment-intents", body, { auth: true, idempotencyKey: input.idempotencyKey });
  }

  getIntent(id: string): Promise<PaymentIntentView> {
    return this.get(`/v1/payment-intents/${id}`, { auth: true });
  }

  listIntents(): Promise<PaymentIntentView[]> {
    return this.get("/v1/payment-intents", { auth: true });
  }

  setWebhookUrl(url: string): Promise<void> {
    return this.put("/v1/webhook-url", { url }, { auth: true });
  }

  checkout(sessionId: string): Promise<CheckoutView> {
    return this.get(`/v1/checkout/${sessionId}`);
  }

  startSiwe(sessionId: string, address: string): Promise<{ nonce: string; message: string; expires_at: string }> {
    return this.post(`/v1/checkout/${sessionId}/siwe/challenge`, { address });
  }

  verifySiwe(
    sessionId: string,
    message: string,
    signature: string,
  ): Promise<SiweVerifyResponse> {
    return this.post(`/v1/checkout/${sessionId}/siwe/verify`, { message, signature });
  }

  /**
   * Signs in with a token from `authorize(..., { rememberDevice: true })` instead of
   * a wallet signature. Returns null when the token is unknown or expired.
   */
  async resumeDevice(sessionId: string, deviceToken: string): Promise<SiweVerifyResponse | null> {
    try {
      return await this.post<SiweVerifyResponse>(`/v1/checkout/${sessionId}/device/resume`, {
        device_token: deviceToken,
      });
    } catch (e) {
      if (e instanceof CheckoutApiError && e.status === 401 && e.code === "device_unknown") return null;
      throw e;
    }
  }

  forgetDevice(deviceToken: string): Promise<ForgetResponse> {
    return this.post("/v1/devices/forget", { device_token: deviceToken });
  }

  sourceAssets(sessionId: string): Promise<{ assets: SourceAssetView[]; selected: string | null }> {
    return this.get(`/v1/checkout/${sessionId}/source-assets`);
  }

  /** Re-prices the attempt. Empty `max_debit` means no preview; authorize will refuse. */
  selectSourceAsset(
    sessionId: string,
    assetId: string,
  ): Promise<{ attempt_id: string; source_asset: string; quoted_input: string; max_debit: string }> {
    return this.post(`/v1/checkout/${sessionId}/source-asset`, { asset_id: assetId });
  }

  enrollTotp(sessionId: string): Promise<{ wallet_credential_id: string; otpauth_uri: string; expires_at: string }> {
    return this.post(`/v1/checkout/${sessionId}/totp/enroll`, {});
  }

  /** Drops the signed-in wallet's authenticator so a new one can be enrolled. */
  resetTotp(sessionId: string): Promise<void> {
    return this.post(`/v1/checkout/${sessionId}/totp/reset`, {});
  }

  confirmTotp(sessionId: string, code: string): Promise<void> {
    return this.post(`/v1/checkout/${sessionId}/totp/confirm`, { code });
  }

  /** Read-only EIP-7702 status of the signed-in wallet on each source chain. */
  delegation(sessionId: string): Promise<DelegationStatusResponse> {
    return this.get(`/v1/checkout/${sessionId}/delegation`);
  }

  /**
   * Allowance payload for `eth_signTypedData_v4`. Omitted fields default to this
   * payment's max debit, 5 uses and 7 days.
   */
  prepareSessionGrant(sessionId: string, allowance: PrepareGrantRequest = {}): Promise<SessionGrantView> {
    return this.post(`/v1/checkout/${sessionId}/session-grant`, allowance);
  }

  enableSessionGrant(sessionId: string, grantId: string, signature: string): Promise<AllowanceView> {
    return this.post(`/v1/checkout/${sessionId}/session-grant/enable`, { grant_id: grantId, signature });
  }

  /** The signed-in wallet's active allowance for the selected source, or null. */
  async sessionGrantStatus(sessionId: string): Promise<AllowanceView | null> {
    try {
      return await this.get<AllowanceView>(`/v1/checkout/${sessionId}/session-grant`);
    } catch (e) {
      if (e instanceof CheckoutApiError && e.status === 404 && e.code === "no_allowance") return null;
      throw e;
    }
  }

  revokeSessionGrant(sessionId: string): Promise<RevokeResponse> {
    return this.post(`/v1/checkout/${sessionId}/session-grant/revoke`, {});
  }

  /** Payer: start an autopay mandate with a TOTP code; returns typed data to sign. */
  setupAutopay(sessionId: string, input: MandateSetupRequest): Promise<MandateSetupView> {
    return this.post(`/v1/checkout/${sessionId}/autopay`, input);
  }

  enableAutopay(sessionId: string, mandateId: string, grantId: string, signature: string): Promise<MandateView> {
    return this.post(`/v1/checkout/${sessionId}/autopay/${mandateId}/enable`, { grant_id: grantId, signature });
  }

  /** Payer: every mandate the signed-in wallet has signed. */
  payerMandates(sessionId: string): Promise<MandateView[]> {
    return this.get(`/v1/checkout/${sessionId}/autopay`);
  }

  revokeAutopay(sessionId: string, mandateId: string): Promise<MandateView> {
    return this.post(`/v1/checkout/${sessionId}/autopay/${mandateId}/revoke`, {});
  }

  /** Merchant: active, expired, exhausted and revoked mandates. */
  mandates(): Promise<MandateView[]> {
    return this.get("/v1/autopay", { auth: true });
  }

  /** Merchant: charge a mandate. Replays with the same key return the first result. */
  chargeMandate(
    mandateId: string,
    input: { merchantOrderId: string; outputAmount: string; idempotencyKey: string },
  ): Promise<ChargeView> {
    return this.post(
      `/v1/autopay/${mandateId}/charges`,
      { merchant_order_id: input.merchantOrderId, output_amount: input.outputAmount },
      { auth: true, idempotencyKey: input.idempotencyKey },
    );
  }

  /** Invite-gated merchant signup. Credentials in the response are shown once. */
  signup(input: { name: string; payout: PayoutRequest }, signupToken: string): Promise<SignupResponse> {
    return this.send("POST", "/v1/merchants", input, { headers: { "X-Signup-Token": signupToken } });
  }

  rotateApiKey(): Promise<ApiKeyResponse> {
    return this.post("/v1/api-keys/rotate", {}, { auth: true });
  }

  rotateWebhookSecret(): Promise<WebhookSecretResponse> {
    return this.post("/v1/webhook-secret/rotate", {}, { auth: true });
  }

  payoutConfig(): Promise<PayoutView> {
    return this.get("/v1/payout-config", { auth: true });
  }

  setPayoutConfig(payout: PayoutRequest): Promise<void> {
    return this.put("/v1/payout-config", payout, { auth: true });
  }

  authorize(
    sessionId: string,
    code: string,
    attemptId?: string,
    opts: { rememberDevice?: boolean } = {},
  ): Promise<AuthorizeResponse> {
    const body: Record<string, string | boolean> = { code };
    if (attemptId !== undefined) body["attempt_id"] = attemptId;
    if (opts.rememberDevice) body["remember_device"] = true;
    return this.post(`/v1/checkout/${sessionId}/authorize`, body);
  }

  private headers(opts?: SendOptions & { json?: boolean }): HeadersInit {
    const headers: Record<string, string> = { Accept: "application/json", ...opts?.headers };
    if (opts?.json) headers["content-type"] = "application/json";
    if (opts?.auth) {
      if (!this.apiKey) throw new CheckoutApiError(401, "merchant API key is required for this method");
      headers.Authorization = `Bearer ${this.apiKey}`;
    }
    if (opts?.idempotencyKey) headers["Idempotency-Key"] = opts.idempotencyKey;
    return headers;
  }

  private async get<T>(path: string, opts?: { auth?: boolean }): Promise<T> {
    return this.send<T>("GET", path, undefined, opts);
  }

  private async post<T>(path: string, body: unknown, opts?: { auth?: boolean; idempotencyKey?: string }): Promise<T> {
    return this.send<T>("POST", path, body, opts);
  }

  private async put<T>(path: string, body: unknown, opts?: { auth?: boolean }): Promise<T> {
    return this.send<T>("PUT", path, body, opts);
  }

  private async send<T>(method: string, path: string, body?: unknown, opts?: SendOptions): Promise<T> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      const init: RequestInit = {
        method,
        signal: controller.signal,
        headers: this.headers({ ...opts, json: body !== undefined && method !== "GET" }),
      };
      if (body !== undefined && method !== "GET") {
        init.body = JSON.stringify(body);
      }
      const res = await this.fetchImpl(`${this.baseUrl}${path}`, init);
      if (res.status === 204 || res.status === 200 && res.headers.get("content-length") === "0") {
        return undefined as T;
      }
      if (!res.ok) {
        const body = (await res.json().catch(() => null)) as { code?: unknown; error?: unknown } | null;
        const code = typeof body?.code === "string" ? body.code : undefined;
        const reason = typeof body?.error === "string" && body.error ? body.error : `${method} ${path} failed with ${res.status}`;
        throw new CheckoutApiError(res.status, reason, code);
      }
      const text = await res.text();
      if (!text) return undefined as T;
      return JSON.parse(text) as T;
    } finally {
      clearTimeout(timer);
    }
  }
}

/** Hosted checkout URLs the browser SDK is allowed to open. */
export function isAllowedCheckoutUrl(url: string): boolean {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return false;
    if (parsed.username || parsed.password) return false;
    return parsed.hostname.length > 0;
  } catch {
    return false;
  }
}

/**
 * Open the hosted checkout in a popup. Merchant API keys must not be passed
 * into this function or into the opened document.
 */
export type MerchantContinuation = "success" | "failure" | "pending";

/**
 * What the merchant app should do when the payer's browser comes back.
 * `success` and `failure` are terminal. `pending` means keep the order open
 * and confirm the intent with the merchant API key before fulfilling it.
 */
export function merchantContinuation(input: {
  intentStatus: string;
  attemptStatus?: string | null;
  execution?: string | null;
}): MerchantContinuation {
  if (input.intentStatus === "succeeded" || input.attemptStatus === "settled") return "success";
  if (
    input.attemptStatus === "failed" ||
    input.attemptStatus === "refunded" ||
    input.attemptStatus === "expired" ||
    input.execution === "simulation_failed" ||
    input.execution === "execute_rejected"
  ) {
    return "failure";
  }
  return "pending";
}

/** Append the continuation fields to the merchant's success URL. Existing query keys are kept. */
export function merchantReturnUrl(
  successUrl: string,
  pageBase: string,
  fields: {
    intentId: string;
    sessionId: string;
    result: MerchantContinuation;
    intentStatus: string;
    attemptStatus?: string | null;
    execution?: string | null;
  },
): string {
  const url = new URL(successUrl, pageBase);
  url.searchParams.set("pcc_intent", fields.intentId);
  url.searchParams.set("pcc_session", fields.sessionId);
  url.searchParams.set("pcc_result", fields.result);
  url.searchParams.set("pcc_status", fields.intentStatus);
  if (fields.attemptStatus) url.searchParams.set("pcc_attempt_status", fields.attemptStatus);
  if (fields.execution) url.searchParams.set("pcc_execution", fields.execution);
  if (successUrl.startsWith("/") && !successUrl.startsWith("//")) return `${url.pathname}${url.search}${url.hash}`;
  return url.toString();
}

const MODAL_ID = "pcc-checkout-modal";

/** Tachyon tokens used by the overlay. Keep in sync with `docs/ui/tachyon.md`. */
const MODAL = {
  overlay: "oklch(0% 0 0 / 0.72)",
  sheet: "oklch(5% 0 0)",
  closeBg: "oklch(15% 0 0)",
  border: "oklch(20% 0 0)",
  text: "oklch(95% 0 0)",
  radius: "8px",
  elevation: "0 24px 80px rgba(0, 0, 0, 0.45)",
  space5: "16px",
  space3: "8px",
  touch: "44px",
} as const;

function restoreBodyScroll(root: Element | null): void {
  if (typeof document === "undefined" || !root) return;
  document.body.style.overflow = root.getAttribute("data-prev-overflow") ?? "";
}

export function closeHostedCheckout(): void {
  if (typeof document === "undefined") return;
  const root = document.getElementById(MODAL_ID);
  restoreBodyScroll(root);
  root?.remove();
}

export interface OpenCheckoutOptions {
  /** Overlay iframe on the merchant page (default). `popup` is a fallback for wallets that refuse iframes. */
  mode?: "modal" | "popup";
}

/**
 * Open hosted checkout. Default is a modal overlay (iframe) following Blade Modal
 * behaviour: Escape, backdrop click, focus return. Popup is `{ mode: "popup" }`.
 * Merchant API keys must not be passed into this function or the opened document.
 */
export function openHostedCheckout(checkoutUrl: string, opts: OpenCheckoutOptions = {}): { close: () => void } {
  if (typeof window === "undefined") {
    throw new CheckoutApiError(400, "openHostedCheckout requires a browser");
  }
  if (!isAllowedCheckoutUrl(checkoutUrl)) {
    throw new CheckoutApiError(400, "checkout URL is not an allowed origin");
  }
  if (opts.mode === "popup") {
    const win = window.open(checkoutUrl, "pcc-checkout", "popup,noopener,noreferrer,width=420,height=760");
    return { close: () => win?.close() };
  }
  closeHostedCheckout();
  const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const root = document.createElement("div");
  root.id = MODAL_ID;
  root.setAttribute("role", "dialog");
  root.setAttribute("aria-modal", "true");
  root.setAttribute("aria-label", "Checkout");
  root.setAttribute("data-prev-overflow", document.body.style.overflow);
  document.body.style.overflow = "hidden";
  root.style.cssText = `position:fixed;inset:0;z-index:2147483646;display:grid;place-items:center;padding:${MODAL.space5};background:${MODAL.overlay}`;
  const sheet = document.createElement("div");
  sheet.style.cssText = `position:relative;width:min(420px,100%);height:min(760px,92vh);background:${MODAL.sheet};border-radius:${MODAL.radius};overflow:hidden;box-shadow:${MODAL.elevation}`;
  const close = document.createElement("button");
  close.type = "button";
  close.setAttribute("aria-label", "Close checkout");
  close.textContent = "×";
  close.style.cssText = `position:absolute;top:${MODAL.space3};right:${MODAL.space3};z-index:1;width:${MODAL.touch};height:${MODAL.touch};padding:0;border:1px solid ${MODAL.border};border-radius:0;background:${MODAL.closeBg};color:${MODAL.text};cursor:pointer;font:500 20px/1 Inter,system-ui,sans-serif`;
  const frame = document.createElement("iframe");
  frame.title = "Tachyon checkout";
  const src = new URL(checkoutUrl);
  src.searchParams.set("pcc_embed", "1");
  frame.src = src.toString();
  frame.allow = "clipboard-write; publickey-credentials-get";
  frame.style.cssText = `width:100%;height:100%;border:0;background:${MODAL.sheet}`;
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      dismiss();
      return;
    }
    if (e.key !== "Tab" || !root.isConnected) return;
    const cycle = [close, frame];
    const i = cycle.indexOf(document.activeElement as HTMLButtonElement | HTMLIFrameElement);
    if (e.shiftKey && i <= 0) {
      e.preventDefault();
      frame.focus();
    } else if (!e.shiftKey && (i === cycle.length - 1 || i < 0)) {
      e.preventDefault();
      close.focus();
    }
  };
  const onMessage = (e: MessageEvent) => {
    const data = e.data as { source?: string; kind?: string } | null;
    if (data?.source === "pcc-checkout" && data.kind === "done") dismiss();
  };
  const dismiss = () => {
    document.removeEventListener("keydown", onKey);
    window.removeEventListener("message", onMessage);
    closeHostedCheckout();
    previouslyFocused?.focus();
  };
  close.onclick = dismiss;
  document.addEventListener("keydown", onKey);
  window.addEventListener("message", onMessage);
  if (typeof MutationObserver !== "undefined") {
    const observer = new MutationObserver(() => {
      if (!document.getElementById(MODAL_ID)) {
        document.removeEventListener("keydown", onKey);
        window.removeEventListener("message", onMessage);
        observer.disconnect();
      }
    });
    observer.observe(document.body, { childList: true });
  }
  root.addEventListener("click", (e) => {
    if (e.target === root) dismiss();
  });
  const media = document.createElement("style");
  media.textContent = `@media (max-width: 480px){#${MODAL_ID}{padding:0!important;align-items:stretch!important}#${MODAL_ID}>div{width:100%!important;height:100%!important;max-height:none!important;border-radius:0!important}}@media (max-height: 560px) and (min-width: 481px){#${MODAL_ID}{padding:0!important}#${MODAL_ID}>div{width:min(420px,100%)!important;height:100%!important;max-height:none!important;border-radius:0!important}}`;
  sheet.append(close, frame);
  root.append(media, sheet);
  document.body.append(root);
  close.focus();
  return { close: dismiss };
}
