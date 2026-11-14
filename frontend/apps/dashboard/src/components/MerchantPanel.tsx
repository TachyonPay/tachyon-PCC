"use client";

import {
  CheckoutApiError,
  CheckoutClient,
  formatBaseUnits,
  isAllowedCheckoutUrl,
  openHostedCheckout,
  parseBaseUnits,
  type MandateView,
  type PaymentIntentView,
  type PayoutRequest,
} from "@pcc/checkout-sdk";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { DEFAULT_PAYOUT_ASSET, PAYOUT_ASSETS, matchPayout, payoutAssetFor } from "../lib/payoutAssets";
import { BrandMark } from "./BrandMark";
import { DashboardShell, type DashNavItem } from "./DashboardShell";

const API = "";

const EMPTY_PAYOUT: PayoutRequest = {
  receiving_address: "",
  chain_id: DEFAULT_PAYOUT_ASSET.chainId,
  decimals: DEFAULT_PAYOUT_ASSET.decimals,
  one_click_asset_id: DEFAULT_PAYOUT_ASSET.assetId,
  token_address: DEFAULT_PAYOUT_ASSET.token,
};

function describe(e: unknown, fallback: string): string {
  if (e instanceof CheckoutApiError) return `${fallback}: ${e.message}${e.code ? ` (${e.code})` : ""}`;
  return `${fallback}: ${e instanceof Error ? e.message : "API unreachable"}`;
}

interface Revealed {
  apiKey?: string;
  webhookSecret?: string;
}

interface PaymentRow extends PaymentIntentView {
  product?: string | undefined;
  settlement?: string | undefined;
}

const STOREFRONT_ORDERS = "http://127.0.0.1:3002/api/orders";
const STOREFRONT_PAYOUT = "http://127.0.0.1:3002/api/payout";

interface StorefrontOrder {
  id?: string;
  merchant_order_id: string;
  status: string;
  output_amount: string;
  product?: string;
  settle_chain?: string;
  checkout_session_id?: string;
  checkout_url?: string;
  expires_at?: string;
}

function mergePayments(api: PaymentIntentView[], storefront: StorefrontOrder[]): PaymentRow[] {
  const rows = new Map<string, PaymentRow>();
  for (const order of storefront) {
    const id = order.id || order.merchant_order_id;
    rows.set(id, {
      id,
      merchant_id: "storefront",
      merchant_order_id: order.merchant_order_id,
      status: order.status,
      output_amount: order.output_amount,
      expires_at: order.expires_at ?? new Date().toISOString(),
      checkout_session_id: order.checkout_session_id ?? "",
      checkout_url: order.checkout_url ?? "",
      product: order.product,
      settlement: order.status === "succeeded" ? `Received on ${order.settle_chain ?? "Arbitrum"}` : undefined,
    });
  }
  for (const intent of api) {
    const prev = rows.get(intent.id);
    rows.set(intent.id, {
      ...intent,
      product: prev?.product,
      settlement:
        intent.status === "succeeded" ? (prev?.settlement ?? "Received on Arbitrum") : prev?.settlement,
    });
  }
  return [...rows.values()].sort((a, b) => b.merchant_order_id.localeCompare(a.merchant_order_id));
}

const NAV: DashNavItem[] = [
  { id: "pay", label: "Payments", group: "Collect", icon: "pay" },
  { id: "autopay", label: "Autopay", group: "Collect", icon: "autopay" },
  { id: "payout", label: "Payouts", group: "Money", icon: "payout" },
  { id: "account", label: "Account", group: "Account", icon: "account" },
  { id: "signup", label: "New merchant", group: "Account", icon: "signup" },
  { id: "operator", label: "Operator", href: "/operator", icon: "operator" },
];

type Tab = "pay" | "autopay" | "payout" | "account" | "signup";

const TITLES: Record<Tab, string> = {
  pay: "Payments",
  autopay: "Autopay",
  payout: "Payouts",
  account: "Account",
  signup: "New merchant",
};

function statusBadge(status: string): string {
  if (status === "succeeded" || status === "active") return "dash-badge dash-badge-positive";
  if (status === "failed" || status === "expired" || status === "revoked") return "dash-badge dash-badge-negative";
  if (status === "processing") return "dash-badge dash-badge-notice";
  return "dash-badge";
}

function statusLabel(status: string): string {
  if (status === "requires_payment") return "Open";
  if (status === "succeeded") return "Captured";
  return status.replaceAll("_", " ");
}

export function MerchantPanel({ testMode = true }: { testMode?: boolean }) {
  const keyRef = useRef<HTMLInputElement>(null);
  const inviteRef = useRef<HTMLInputElement>(null);
  const [tab, setTab] = useState<Tab>("pay");
  const [name, setName] = useState<string>("");
  const [signupName, setSignupName] = useState("");
  const [webhookUrl, setWebhookUrl] = useState("");
  const [payout, setPayout] = useState<PayoutRequest>(EMPTY_PAYOUT);
  const [revealed, setRevealed] = useState<Revealed | null>(null);
  const [orderId, setOrderId] = useState("order-1");
  const [amount, setAmount] = useState("0.20");
  const [intents, setIntents] = useState<PaymentRow[]>([]);
  const [mandates, setMandates] = useState<MandateView[]>([]);
  const [chargeAmount, setChargeAmount] = useState("0.20");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [sessionOpen, setSessionOpen] = useState(false);
  const [collectOpen, setCollectOpen] = useState(false);
  const collectOrderRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const [ordersRes, payoutRes] = await Promise.all([
        fetch(STOREFRONT_ORDERS, { headers: { accept: "application/json" } }).catch(() => null),
        fetch(STOREFRONT_PAYOUT, { headers: { accept: "application/json" } }).catch(() => null),
      ]);
      if (cancelled) return;
      if (ordersRes?.ok) {
        const body = (await ordersRes.json()) as { orders?: StorefrontOrder[] };
        if (!cancelled && (body.orders?.length ?? 0) > 0) setIntents(mergePayments([], body.orders ?? []));
      }
      if (payoutRes?.ok) {
        const body = (await payoutRes.json()) as PayoutRequest & { merchant?: string };
        if (cancelled) return;
        if (body.receiving_address) setPayout({ ...body, token_address: body.token_address ?? null });
        if (body.merchant) setName(body.merchant);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const clientFromField = useCallback(() => {
    const apiKey = keyRef.current?.value.trim() ?? "";
    if (!apiKey) return null;
    return new CheckoutClient({ baseUrl: API, apiKey, timeoutMs: 8_000 });
  }, []);

  const withClient = useCallback(
    async (fallback: string, body: (client: CheckoutClient) => Promise<void>) => {
      const client = clientFromField();
      if (!client) {
        setError("Enter a merchant API key.");
        return;
      }
      setError(null);
      setNotice(null);
      try {
        await body(client);
      } catch (e) {
        setError(describe(e, fallback));
      }
    },
    [clientFromField],
  );

  const useKey = (apiKey: string) => {
    if (keyRef.current) keyRef.current.value = apiKey;
  };

  const refresh = useCallback(
    () =>
      withClient("Invalid API key or API down", async (client) => {
        const me = await client.me();
        setName(me.name);
        setWebhookUrl(me.webhook_url ?? "");
        const listed = await client.listIntents();
        const storefront = await fetch(STOREFRONT_ORDERS, { headers: { accept: "application/json" } })
          .then(async (r) => (r.ok ? ((await r.json()) as { orders?: StorefrontOrder[] }) : { orders: [] }))
          .catch(() => ({ orders: [] as StorefrontOrder[] }));
        setIntents(mergePayments(listed, storefront.orders ?? []));
        setMandates(await client.mandates());
        const current = await client.payoutConfig().catch(() => null);
        if (current) setPayout({ ...current, token_address: current.token_address ?? null });
        if (sessionOpen) setTab("pay");
        setSessionOpen(false);
      }),
    [sessionOpen, withClient],
  );

  useEffect(() => {
    if (!collectOpen) return;
    collectOrderRef.current?.focus();
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setCollectOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      document.body.style.overflow = prev;
      window.removeEventListener("keydown", onKey);
    };
  }, [collectOpen]);

  useEffect(() => {
    if (!sessionOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSessionOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sessionOpen]);

  const charge = useCallback(
    (mandateId: string) =>
      withClient("Charge failed", async (client) => {
        const units = parseBaseUnits(chargeAmount, payout.decimals);
        if (!units) throw new Error("Charge amount must look like 0.20");
        const out = await client.chargeMandate(mandateId, {
          merchantOrderId: `autopay-${Date.now()}`,
          outputAmount: units,
          idempotencyKey: crypto.randomUUID(),
        });
        setNotice(`Charge ${out.execution}; ${out.charges_remaining} left on this mandate.`);
        setMandates(await client.mandates());
        setIntents(await client.listIntents());
      }),
    [chargeAmount, payout.decimals, withClient],
  );

  const signup = useCallback(async () => {
    const invite = inviteRef.current?.value.trim() ?? "";
    if (!invite) {
      setError("Enter the signup invite token.");
      return;
    }
    if (!payout.receiving_address.trim()) {
      setError("Enter the receiving address. This is where the merchant gets paid.");
      return;
    }
    setError(null);
    setNotice(null);
    try {
      const out = await new CheckoutClient({ baseUrl: API, timeoutMs: 8_000 }).signup({ name: signupName, payout }, invite);
      if (inviteRef.current) inviteRef.current.value = "";
      useKey(out.api_key);
      setName(out.name);
      setRevealed({ apiKey: out.api_key, webhookSecret: out.webhook_secret });
      setIntents([]);
      setTab("pay");
    } catch (e) {
      setError(describe(e, "Signup failed"));
    }
  }, [payout, signupName]);

  const rotateKey = useCallback(
    () =>
      withClient("Key rotation failed", async (client) => {
        const out = await client.rotateApiKey();
        useKey(out.api_key);
        setRevealed({ apiKey: out.api_key });
      }),
    [withClient],
  );

  const rotateSecret = useCallback(
    () =>
      withClient("Secret rotation failed", async (client) => {
        const out = await client.rotateWebhookSecret();
        setRevealed({ webhookSecret: out.webhook_secret });
      }),
    [withClient],
  );

  const savePayout = useCallback(
    () =>
      withClient("Payout update failed", async (client) => {
        await client.setPayoutConfig({ ...payout, token_address: payout.token_address || null });
        setNotice("Payout destination saved.");
      }),
    [payout, withClient],
  );

  const saveWebhook = useCallback(
    () =>
      withClient("Webhook URL rejected", async (client) => {
        await client.setWebhookUrl(webhookUrl);
        setNotice("Webhook URL saved.");
      }),
    [webhookUrl, withClient],
  );

  const create = useCallback(
    () =>
      withClient("Create failed", async (client) => {
        const units = parseBaseUnits(amount, payout.decimals);
        if (!units) throw new Error("Amount must look like 0.20");
        const intent = await client.createIntent({
          merchantOrderId: orderId,
          outputAmount: units,
          idempotencyKey: `${orderId}-${Date.now()}`,
        });
        setIntents((prev) => [intent, ...prev]);
        setCollectOpen(false);
        if (isAllowedCheckoutUrl(intent.checkout_url)) openHostedCheckout(intent.checkout_url);
      }),
    [amount, orderId, payout.decimals, withClient],
  );

  const chosen = matchPayout(payout.chain_id, payout.one_click_asset_id);
  const networks = [...new Map(PAYOUT_ASSETS.map((a) => [a.chainId, a.network])).entries()];
  const tokensOnNetwork = PAYOUT_ASSETS.filter((a) => a.chainId === chosen.chainId);
  const chooseAsset = (asset: (typeof PAYOUT_ASSETS)[number]) =>
    setPayout((p) => ({
      ...p,
      chain_id: asset.chainId,
      decimals: asset.decimals,
      token_address: asset.token,
      one_click_asset_id: asset.assetId,
    }));
  const payoutChoice = (
    <>
      <label className="dash-field">
        Receiving address
        <input
          value={payout.receiving_address}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => setPayout((p) => ({ ...p, receiving_address: e.target.value }))}
        />
      </label>
      <label className="dash-field">
        Network
        <select
          value={chosen.chainId}
          onChange={(e) => {
            const next = payoutAssetFor(Number(e.target.value), chosen.symbol);
            if (next) chooseAsset(next);
          }}
        >
          {networks.map(([chainId, network]) => (
            <option key={chainId} value={chainId}>
              {network}
            </option>
          ))}
        </select>
      </label>
      <label className="dash-field">
        Token
        <select
          value={chosen.symbol}
          onChange={(e) => {
            const next = PAYOUT_ASSETS.find((a) => a.chainId === chosen.chainId && a.symbol === e.target.value);
            if (next) chooseAsset(next);
          }}
        >
          {tokensOnNetwork.map((asset) => (
            <option key={asset.symbol} value={asset.symbol}>
              {asset.symbol}
            </option>
          ))}
        </select>
      </label>
    </>
  );

  const stats = useMemo(() => {
    let volume = 0n;
    let captured = 0;
    let open = 0;
    for (const intent of intents) {
      if (/^[0-9]+$/.test(intent.output_amount)) volume += BigInt(intent.output_amount);
      if (intent.status === "succeeded") captured += 1;
      else open += 1;
    }
    return {
      volume: formatBaseUnits(volume.toString(), chosen.decimals),
      captured,
      open,
      mandates: mandates.filter((m) => m.status === "active").length,
    };
  }, [chosen.decimals, intents, mandates]);

  return (
    <DashboardShell
      nav={NAV}
      current={tab}
      onNav={(id) => {
        if (id === "operator") return;
        setError(null);
        setTab(id as Tab);
      }}
      title={TITLES[tab]}
      testMode={testMode}
      actions={
        tab === "pay" ? (
          <button type="button" className="dash-btn" onClick={() => setCollectOpen(true)}>
            + Collect payment
          </button>
        ) : null
      }
      session={
        <div className="dash-session">
          {name ? (
            <button type="button" className="dash-user" onClick={() => void refresh()} title="Refresh">
              <span className="dash-avatar" aria-hidden>
                {name.slice(0, 1).toUpperCase()}
              </span>
              <span className="dash-session-name">{name}</span>
            </button>
          ) : (
            <button
              type="button"
              className="dash-btn dash-btn-secondary"
              aria-expanded={sessionOpen}
              onClick={() => setSessionOpen((open) => !open)}
            >
              Sign in
            </button>
          )}
          <div
            className={sessionOpen ? "dash-session-menu" : "dash-sr"}
            role={sessionOpen ? "dialog" : undefined}
            aria-label={sessionOpen ? "Merchant sign in" : undefined}
          >
            <label className="dash-field">
              Merchant API key
              <input ref={keyRef} type="password" autoComplete="off" defaultValue="" placeholder="pcc_sk_…" />
            </label>
            {sessionOpen ? (
              <button type="button" className="dash-btn" onClick={() => void refresh()}>
                Continue
              </button>
            ) : null}
          </div>
        </div>
      }
    >
      {error ? (
        <p role="alert" className="dash-alert">
          {error}
        </p>
      ) : null}
      {notice ? (
        <p role="status" className="dash-ok">
          {notice}
        </p>
      ) : null}
      {revealed ? (
        <div className="dash-secret" role="status">
          <p>
            <strong>Copy now. Shown once and not stored on this page.</strong>
          </p>
          {revealed.apiKey ? (
            <p>
              API key <code>{revealed.apiKey}</code>
            </p>
          ) : null}
          {revealed.webhookSecret ? (
            <p>
              Webhook secret <code>{revealed.webhookSecret}</code>
            </p>
          ) : null}
          <button type="button" className="dash-btn" onClick={() => setRevealed(null)}>
            I saved them
          </button>
        </div>
      ) : null}

      {tab === "pay" ? (
        <>
          <div className="dash-stats">
            <article className="dash-stat">
              <p className="dash-stat-label">Volume</p>
              <p className="dash-stat-value">
                {stats.volume} <span>{chosen.symbol}</span>
              </p>
            </article>
            <article className="dash-stat">
              <p className="dash-stat-label">Captured</p>
              <p className="dash-stat-value">{stats.captured}</p>
            </article>
            <article className="dash-stat">
              <p className="dash-stat-label">Open</p>
              <p className="dash-stat-value">{stats.open}</p>
            </article>
            <article className="dash-stat">
              <p className="dash-stat-label">Active mandates</p>
              <p className="dash-stat-value">{stats.mandates}</p>
            </article>
          </div>
          {intents.length === 0 ? (
            <div className="dash-empty">
              <img src="/logo.svg" alt="" width={40} height={40} />
              <h2>No payments yet</h2>
              <p>Collect a payment to open hosted checkout. Minimum about 0.15 USDC.</p>
              <button type="button" className="dash-btn" onClick={() => setCollectOpen(true)}>
                + Collect payment
              </button>
            </div>
          ) : (
            <div className="dash-table-wrap">
              <table className="dash-table">
                <thead>
                  <tr>
                    <th>Order</th>
                    <th>Amount</th>
                    <th>Status</th>
                    <th></th>
                  </tr>
                </thead>
                <tbody>
                  {intents.map((intent) => (
                    <tr key={intent.id}>
                      <td>
                        {intent.product ? <div>{intent.product}</div> : null}
                        <code>{intent.merchant_order_id}</code>
                      </td>
                      <td className="dash-amount">{formatBaseUnits(intent.output_amount, chosen.decimals)} {chosen.symbol}</td>
                      <td>
                        <span className={statusBadge(intent.status)}>{statusLabel(intent.status)}</span>
                        {intent.settlement ? <div className="dash-muted">{intent.settlement}</div> : null}
                      </td>
                      <td>
                        {intent.status === "succeeded" ? (
                          <span className="dash-muted">Payout received</span>
                        ) : isAllowedCheckoutUrl(intent.checkout_url) ? (
                          <span className="dash-table-actions">
                            <button type="button" className="dash-btn" onClick={() => openHostedCheckout(intent.checkout_url)}>
                              Open
                            </button>
                            <button
                              type="button"
                              className="dash-btn dash-btn-secondary"
                              onClick={() => openHostedCheckout(intent.checkout_url, { mode: "popup" })}
                            >
                              Popup
                            </button>
                          </span>
                        ) : (
                          <span className="dash-muted">blocked</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      ) : null}

      {tab === "autopay" ? (
        <>
          <p className="dash-lead">Payers set these up in checkout. Charging needs no payer action.</p>
          {mandates.length > 0 ? (
            <div className="dash-create">
              <label className="dash-field">
                Charge amount ({chosen.symbol})
                <input value={chargeAmount} onChange={(e) => setChargeAmount(e.target.value)} inputMode="decimal" />
              </label>
            </div>
          ) : null}
          <div className="dash-table-wrap">
            <table className="dash-table">
              <thead>
                <tr>
                  <th>Payer</th>
                  <th>Cap</th>
                  <th>Charges</th>
                  <th>Status</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {mandates.length === 0 ? (
                  <tr>
                    <td colSpan={5} className="dash-muted">
                      No mandates yet.
                    </td>
                  </tr>
                ) : (
                  mandates.map((m) => (
                    <tr key={m.mandate_id}>
                      <td>
                        <code>{m.owner.slice(0, 10)}…</code>
                      </td>
                      <td className="dash-amount">{formatBaseUnits(m.max_amount, chosen.decimals)} {chosen.symbol}</td>
                      <td>
                        {m.charges_remaining}/{m.charges}
                      </td>
                      <td>
                        <span className={statusBadge(m.status)}>{statusLabel(m.status)}</span>
                      </td>
                      <td>
                        {m.status === "active" ? (
                          <button type="button" className="dash-btn" onClick={() => void charge(m.mandate_id)}>
                            Charge
                          </button>
                        ) : null}
                      </td>
                    </tr>
                  ))
                )}
              </tbody>
            </table>
          </div>
        </>
      ) : null}

      {tab === "payout" ? (
        <div className="dash-card">
          <h2>Recipient settings</h2>
          <p className="dash-lead">
            Change where this merchant gets paid. New merchants enter this address when they sign up. The payer
            can send on another chain. NEAR Intents settles the configured asset here, and the two transfers do
            not share a public counterparty.
          </p>
          {payout.receiving_address ? (
            <p className="dash-ok" role="status">
              Receiving {chosen.symbol} at {payout.receiving_address} on {chosen.network}.
            </p>
          ) : (
            <p className="dash-muted">No payout address saved yet.</p>
          )}
          {payoutChoice}
          <div className="dash-card-footer">
            <button type="button" className="dash-btn" onClick={() => void savePayout()}>
              Save payout
            </button>
          </div>
        </div>
      ) : null}

      {tab === "account" ? (
        <div className="dash-card">
          <h2>Webhooks and keys</h2>
          <label className="dash-field">
            Webhook URL
            <input value={webhookUrl} onChange={(e) => setWebhookUrl(e.target.value)} />
          </label>
          <div className="dash-card-footer">
            <button type="button" className="dash-btn" onClick={() => void saveWebhook()}>
              Save URL
            </button>
            <button type="button" className="dash-btn dash-btn-secondary" onClick={() => void rotateSecret()}>
              Rotate signing secret
            </button>
            <button type="button" className="dash-btn dash-btn-secondary" onClick={() => void rotateKey()}>
              Rotate API key
            </button>
          </div>
        </div>
      ) : null}

      {tab === "signup" ? (
        <div className="dash-card">
          <h2>Create a merchant</h2>
          <p className="dash-muted">Invite only. The receiving address is where this merchant gets paid.</p>
          <label className="dash-field">
            Business name
            <input value={signupName} onChange={(e) => setSignupName(e.target.value)} maxLength={80} />
          </label>
          {payoutChoice}
          <label className="dash-field">
            Invite token
            <input ref={inviteRef} type="text" autoComplete="off" spellCheck={false} name="invite-token" />
          </label>
          <div className="dash-card-footer">
            <button type="button" className="dash-btn" onClick={() => void signup()}>
              Sign up
            </button>
          </div>
        </div>
      ) : null}

      {collectOpen ? (
        <div
          className="dash-modal-backdrop"
          onClick={(e) => {
            if (e.target === e.currentTarget) setCollectOpen(false);
          }}
        >
          <div className="dash-modal" role="dialog" aria-modal="true" aria-labelledby="collect-title">
            <div className="dash-modal-head">
              <BrandMark href={null} />
              <button type="button" className="dash-modal-close" onClick={() => setCollectOpen(false)} aria-label="Close">
                ×
              </button>
            </div>
            <h2 id="collect-title">Collect payment</h2>
            <p className="dash-muted">Opens checkout as a modal. Minimum about 0.15 USDC.</p>
            <label className="dash-field">
              Order id
              <input ref={collectOrderRef} value={orderId} onChange={(e) => setOrderId(e.target.value)} />
            </label>
            <label className="dash-field">
              Amount ({chosen.symbol})
              <input value={amount} onChange={(e) => setAmount(e.target.value)} inputMode="decimal" />
            </label>
            <div className="dash-card-footer">
              <button type="button" className="dash-btn dash-btn-secondary" onClick={() => setCollectOpen(false)}>
                Cancel
              </button>
              <button type="button" className="dash-btn" onClick={() => void create()}>
                Create and open
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </DashboardShell>
  );
}
