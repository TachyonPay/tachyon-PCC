import { describe, expect, it } from "vitest";
import { CheckoutApiError, CheckoutClient, formatBaseUnits, isAllowedCheckoutUrl, merchantContinuation, merchantReturnUrl, openHostedCheckout, parseBaseUnits } from "../src/index.js";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

describe("CheckoutClient", () => {
  it("reads gates with the generated shape", async () => {
    const calls: string[] = [];
    const fetchImpl: typeof fetch = async (url) => {
      calls.push(String(url));
      return json({ dry_run: true, enable_mainnet_transfers: false, mock_authenticator_allowed: false });
    };
    const c = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080/", fetchImpl });
    const g = await c.gates();
    expect(g.dry_run).toBe(true);
    expect(g.enable_mainnet_transfers).toBe(false);
    expect(calls).toEqual(["http://127.0.0.1:8080/v1/meta/gates"]);
  });

  it("surfaces non-2xx as CheckoutApiError", async () => {
    const c = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", fetchImpl: async () => json({}, 503) });
    await expect(c.health()).rejects.toBeInstanceOf(CheckoutApiError);
  });

  it("refuses merchant methods without an API key", async () => {
    const c = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", fetchImpl: async () => json({}) });
    await expect(c.createIntent({ merchantOrderId: "o", outputAmount: "1", idempotencyKey: "k" })).rejects.toMatchObject({
      status: 401,
    });
  });

  it("sends bearer + idempotency on createIntent and never puts the key in the URL", async () => {
    const seen: { url: string; auth: string | null; idem: string | null }[] = [];
    const fetchImpl: typeof fetch = async (url, init) => {
      const headers = new Headers(init?.headers);
      seen.push({ url: String(url), auth: headers.get("authorization"), idem: headers.get("idempotency-key") });
      return json({
        id: "1",
        merchant_id: "m",
        merchant_order_id: "o",
        status: "requires_payment",
        output_amount: "1",
        expires_at: "t",
        checkout_session_id: "s",
        checkout_url: "http://127.0.0.1:3000/checkout/s",
      });
    };
    const c = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", apiKey: "pcc_sk_alice_demo", fetchImpl });
    await c.createIntent({ merchantOrderId: "o", outputAmount: "1", idempotencyKey: "idem-1" });
    expect(seen[0]?.url).toBe("http://127.0.0.1:8080/v1/payment-intents");
    expect(seen[0]?.auth).toBe("Bearer pcc_sk_alice_demo");
    expect(seen[0]?.idem).toBe("idem-1");
  });

  it("loads a public checkout session without an API key", async () => {
    const fetchImpl: typeof fetch = async (url, init) => {
      const headers = new Headers(init?.headers);
      expect(headers.get("authorization")).toBeNull();
      expect(String(url)).toBe("http://127.0.0.1:8080/v1/checkout/sess");
      return json({
        session_id: "sess",
        merchant_name: "Alice",
        merchant_order_id: "o",
        output_amount: "1",
        destination_asset: "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near",
        source_asset: "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near",
        receiving_address: "0x1",
        expires_at: "t",
        attempt_id: null,
        quoted_input: null,
        max_debit: null,
        deposit_address: null,
        attempt_status: null,
        status: "requires_payment",
      });
    };
    const c = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", fetchImpl });
    const view = await c.checkout("sess");
    expect(view.session_id).toBe("sess");
  });
});

describe("hosted checkout URL allow-list", () => {
  it("allows loopback hosted checkout and refuses XSS / open-redirect vectors", () => {
    expect(isAllowedCheckoutUrl("http://127.0.0.1:3000/checkout/abc")).toBe(true);
    expect(isAllowedCheckoutUrl("http://localhost:3000/checkout/abc")).toBe(true);
    expect(isAllowedCheckoutUrl("https://shop.example/checkout/abc")).toBe(true);
    expect(isAllowedCheckoutUrl("javascript:alert(1)")).toBe(false);
    expect(isAllowedCheckoutUrl("data:text/html,<script>alert(1)</script>")).toBe(false);
    expect(isAllowedCheckoutUrl("https://127.0.0.1:3000/checkout/abc")).toBe(true);
    expect(isAllowedCheckoutUrl("http://user:pass@127.0.0.1:3000/checkout/abc")).toBe(false);
    expect(isAllowedCheckoutUrl("http://shop.example/checkout/abc")).toBe(true);
  });

  it("exposes the server error code on CheckoutApiError", async () => {
    const c = new CheckoutClient({
      baseUrl: "http://127.0.0.1:8080",
      fetchImpl: async () => json({ code: "session_grant_required", error: "sign first" }, 409),
    });
    await expect(c.authorize("s", "123456")).rejects.toMatchObject({
      status: 409,
      code: "session_grant_required",
      message: "sign first",
    });
    const plain = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", fetchImpl: async () => new Response("oops", { status: 502 }) });
    await expect(plain.health()).rejects.toMatchObject({ status: 502, code: undefined });
  });

  it("remembered devices resume without a merchant key and map unknown tokens to null", async () => {
    const seen: { url: string; auth: string | null; body: string | null }[] = [];
    const fetchImpl: typeof fetch = async (url, init) => {
      const body = (init?.body as string | undefined) ?? null;
      seen.push({ url: String(url), auth: new Headers(init?.headers).get("authorization"), body });
      if (String(url).endsWith("/device/resume") && body?.includes("stale")) {
        return json({ code: "device_unknown", error: "sign in" }, 401);
      }
      return json({ address: "0xabc", totp_enrolled: true, forgotten: true, device_token: "pcc_dev_x" });
    };
    const c = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", fetchImpl });
    expect((await c.resumeDevice("s", "pcc_dev_ok"))?.address).toBe("0xabc");
    expect(await c.resumeDevice("s", "stale")).toBeNull();
    expect((await c.forgetDevice("pcc_dev_ok")).forgotten).toBe(true);
    expect((await c.authorize("s", "123456", undefined, { rememberDevice: true })).device_token).toBe("pcc_dev_x");
    await c.authorize("s", "123456");
    expect(seen.map((r) => r.url)).toEqual([
      "http://127.0.0.1:8080/v1/checkout/s/device/resume",
      "http://127.0.0.1:8080/v1/checkout/s/device/resume",
      "http://127.0.0.1:8080/v1/devices/forget",
      "http://127.0.0.1:8080/v1/checkout/s/authorize",
      "http://127.0.0.1:8080/v1/checkout/s/authorize",
    ]);
    expect(seen.every((r) => r.auth === null)).toBe(true);
    expect(JSON.parse(seen[0]?.body ?? "{}")).toEqual({ device_token: "pcc_dev_ok" });
    expect(JSON.parse(seen[3]?.body ?? "{}")).toEqual({ code: "123456", remember_device: true });
    expect(JSON.parse(seen[4]?.body ?? "{}")).toEqual({ code: "123456" });
  });

  it("session grant and delegation use public checkout routes without a merchant key", async () => {
    const seen: { method: string; url: string; auth: string | null; body: string | null }[] = [];
    const fetchImpl: typeof fetch = async (url, init) => {
      seen.push({
        method: init?.method ?? "GET",
        url: String(url),
        auth: new Headers(init?.headers).get("authorization"),
        body: (init?.body as string | undefined) ?? null,
      });
      return json({ grant_id: "g", typed_data: {}, enabled: true, chains: [] });
    };
    const c = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", fetchImpl });
    await c.delegation("sess");
    await c.prepareSessionGrant("sess");
    await c.prepareSessionGrant("sess", { max_amount: "2000000", uses: 3, valid_days: 7 });
    await c.enableSessionGrant("sess", "g", "0xsig");
    await c.sessionGrantStatus("sess");
    await c.revokeSessionGrant("sess");
    expect(seen.map((s) => `${s.method} ${s.url}`)).toEqual([
      "GET http://127.0.0.1:8080/v1/checkout/sess/delegation",
      "POST http://127.0.0.1:8080/v1/checkout/sess/session-grant",
      "POST http://127.0.0.1:8080/v1/checkout/sess/session-grant",
      "POST http://127.0.0.1:8080/v1/checkout/sess/session-grant/enable",
      "GET http://127.0.0.1:8080/v1/checkout/sess/session-grant",
      "POST http://127.0.0.1:8080/v1/checkout/sess/session-grant/revoke",
    ]);
    expect(seen.every((s) => s.auth === null)).toBe(true);
    expect(JSON.parse(seen[1]!.body!)).toEqual({});
    expect(JSON.parse(seen[2]!.body!)).toEqual({ max_amount: "2000000", uses: 3, valid_days: 7 });
    expect(JSON.parse(seen[3]!.body!)).toEqual({ grant_id: "g", signature: "0xsig" });
  });

  it("autopay: payer routes send no key; merchant routes need the key and an idempotency key", async () => {
    const seen: { method: string; url: string; headers: Headers; body: string | null }[] = [];
    const fetchImpl: typeof fetch = async (url, init) => {
      seen.push({
        method: init?.method ?? "GET",
        url: String(url),
        headers: new Headers(init?.headers),
        body: (init?.body as string | undefined) ?? null,
      });
      return json({});
    };
    const payer = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", fetchImpl });
    await payer.setupAutopay("sess", { code: "123456", charges: 4, interval_hours: 24, valid_days: 30 });
    await payer.enableAutopay("sess", "m1", "g", "0xsig");
    await payer.payerMandates("sess");
    await payer.revokeAutopay("sess", "m1");
    expect(seen.every((s) => s.headers.get("authorization") === null)).toBe(true);
    await expect(payer.mandates()).rejects.toMatchObject({ status: 401 });

    const merchant = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", apiKey: "pcc_sk_x", fetchImpl });
    await merchant.mandates();
    await merchant.chargeMandate("m1", { merchantOrderId: "sub-1", outputAmount: "500000", idempotencyKey: "k1" });
    expect(seen.map((s) => `${s.method} ${s.url.replace("http://127.0.0.1:8080", "")}`)).toEqual([
      "POST /v1/checkout/sess/autopay",
      "POST /v1/checkout/sess/autopay/m1/enable",
      "GET /v1/checkout/sess/autopay",
      "POST /v1/checkout/sess/autopay/m1/revoke",
      "GET /v1/autopay",
      "POST /v1/autopay/m1/charges",
    ]);
    const charge = seen[5]!;
    expect(charge.headers.get("authorization")).toBe("Bearer pcc_sk_x");
    expect(charge.headers.get("idempotency-key")).toBe("k1");
    expect(JSON.parse(charge.body!)).toEqual({ merchant_order_id: "sub-1", output_amount: "500000" });
    expect(JSON.parse(seen[0]!.body!)).toEqual({ code: "123456", charges: 4, interval_hours: 24, valid_days: 30 });
  });

  it("allowance status maps no_allowance to null and keeps other errors", async () => {
    const reply = (status: number, code: string) =>
      new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", fetchImpl: async () => json({ error: "x", code }, status) });
    await expect(reply(404, "no_allowance").sessionGrantStatus("s")).resolves.toBeNull();
    await expect(reply(404, "not_found").sessionGrantStatus("s")).rejects.toMatchObject({ code: "not_found" });
    await expect(reply(401, "siwe_required").sessionGrantStatus("s")).rejects.toMatchObject({ status: 401 });
  });

  it("signup sends the invite token header and never a bearer key", async () => {
    let headers = new Headers();
    const fetchImpl: typeof fetch = async (_url, init) => {
      headers = new Headers(init?.headers);
      return json({ merchant_id: "m", name: "Carol", api_key: "pcc_sk_x", webhook_secret: "whsec_x" }, 201);
    };
    const c = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", fetchImpl });
    const out = await c.signup(
      {
        name: "Carol",
        payout: { receiving_address: "0x4444444444444444444444444444444444444444", chain_id: 42161, decimals: 6, one_click_asset_id: "a" },
      },
      "invite-token",
    );
    expect(out.api_key).toBe("pcc_sk_x");
    expect(headers.get("x-signup-token")).toBe("invite-token");
    expect(headers.get("authorization")).toBeNull();
  });

  it("rotation and payout readback require the merchant key", async () => {
    const c = new CheckoutClient({ baseUrl: "http://127.0.0.1:8080", fetchImpl: async () => json({}) });
    await expect(c.rotateApiKey()).rejects.toMatchObject({ status: 401 });
    await expect(c.rotateWebhookSecret()).rejects.toMatchObject({ status: 401 });
    await expect(c.payoutConfig()).rejects.toMatchObject({ status: 401 });
  });

  it("openHostedCheckout throws on a disallowed URL and does not call window.open", () => {
    const opened: string[] = [];
    const previous = globalThis.window;
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: {
        open: (url: string) => {
          opened.push(url);
          return null;
        },
      },
    });
    try {
      expect(() => openHostedCheckout("javascript:alert(1)")).toThrow(CheckoutApiError);
      expect(opened).toEqual([]);
    } finally {
      Object.defineProperty(globalThis, "window", { configurable: true, value: previous });
    }
  });

  it("popup mode uses window.open", () => {
    const opened: string[] = [];
    const previous = globalThis.window;
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: {
        open: (url: string) => {
          opened.push(url);
          return { close: () => undefined };
        },
      },
    });
    try {
      openHostedCheckout("https://pay.example/checkout/abc", { mode: "popup" });
      expect(opened).toEqual(["https://pay.example/checkout/abc"]);
    } finally {
      Object.defineProperty(globalThis, "window", { configurable: true, value: previous });
    }
  });

  it("formats USDC without floats", () => {
    expect(formatBaseUnits("200000")).toBe("0.2");
    expect(parseBaseUnits("0.20")).toBe("200000");
  });
});

describe("merchant return", () => {
  it("classifies settlement, refunds, and everything still in flight", () => {
    expect(merchantContinuation({ intentStatus: "succeeded", attemptStatus: "settled" })).toBe("success");
    expect(merchantContinuation({ intentStatus: "processing", attemptStatus: "refunded" })).toBe("failure");
    expect(merchantContinuation({ intentStatus: "processing", attemptStatus: "failed" })).toBe("failure");
    expect(merchantContinuation({ intentStatus: "requires_payment", execution: "simulation_failed" })).toBe("failure");
    expect(merchantContinuation({ intentStatus: "processing", attemptStatus: "deposit_broadcast", execution: "awaiting_approval" })).toBe("pending");
    expect(merchantContinuation({ intentStatus: "requires_payment", execution: "gated" })).toBe("pending");
  });

  it("adds the continuation fields without dropping the merchant's own query", () => {
    const absolute = merchantReturnUrl("https://shop.example/orders/9?keep=1", "https://pay.example", {
      intentId: "intent-1",
      sessionId: "sess-1",
      result: "success",
      intentStatus: "succeeded",
      attemptStatus: "settled",
    });
    const parsed = new URL(absolute);
    expect(parsed.origin).toBe("https://shop.example");
    expect(parsed.searchParams.get("keep")).toBe("1");
    expect(parsed.searchParams.get("pcc_result")).toBe("success");
    expect(parsed.searchParams.get("pcc_intent")).toBe("intent-1");
    expect(
      merchantReturnUrl("/thanks", "https://pay.example", {
        intentId: "i",
        sessionId: "s",
        result: "failure",
        intentStatus: "processing",
        attemptStatus: "failed",
      }),
    ).toBe("/thanks?pcc_intent=i&pcc_session=s&pcc_result=failure&pcc_status=processing&pcc_attempt_status=failed");
  });
});
