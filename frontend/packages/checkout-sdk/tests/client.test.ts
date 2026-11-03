import { describe, expect, it } from "vitest";
import { CheckoutApiError, CheckoutClient } from "../src/index.js";

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
});
