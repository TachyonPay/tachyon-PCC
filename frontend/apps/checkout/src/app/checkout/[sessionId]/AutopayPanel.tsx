"use client";

import { CheckoutApiError, type CheckoutClient, type MandateView } from "@pcc/checkout-sdk";
import { chainIdHex, toSignTypedDataV4 } from "@pcc/wallet-onboarding";
import { useCallback, useEffect, useState } from "react";
import type { Eip1193 } from "../../../wallet";

interface Props {
  client: CheckoutClient;
  sessionId: string;
  /** Signed-in wallet; may prompt to connect when the session was resumed from a remembered device. */
  walletFor: () => Promise<{ wallet: Eip1193; address: string }>;
  merchant: string;
  /** Formats source-token base units for display. */
  show: (base: string) => string;
  /** Parses a decimal amount into source-token base units; null when malformed. */
  parse: (value: string) => string | null;
  /** Base units as a plain decimal number, matching what `parse` accepts. */
  toDecimal: (base: string) => string;
  defaultCap: string;
}

function describe(e: unknown): string {
  if (e instanceof CheckoutApiError) return `${e.message}${e.code ? ` (${e.code})` : ""}`;
  return e instanceof Error ? e.message : "Autopay request failed";
}

/**
 * Lets an enrolled payer authorize recurring merchant charges: one TOTP code
 * and one wallet signature, then the merchant charges within the limits with
 * no further prompts. The payer can revoke at any time.
 */
export function AutopayPanel({ client, sessionId, walletFor, merchant, show, parse, toDecimal, defaultCap }: Props) {
  const [mandates, setMandates] = useState<MandateView[]>([]);
  const [cap, setCap] = useState("");
  const [charges, setCharges] = useState(12);
  const [hours, setHours] = useState(24 * 30);
  const [days, setDays] = useState(30);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setMandates(await client.payerMandates(sessionId).catch(() => []));
  }, [client, sessionId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const capBase = cap ? parse(cap) : defaultCap;
  const invalid =
    !capBase || capBase === "0" || charges < 1 || charges > 20 || hours < 0 || hours > 744 || days < 1 || days > 30;

  const act = useCallback(async (body: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await body();
    } catch (e) {
      setError(describe(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const setUp = () =>
    act(async () => {
      if (!capBase) throw new Error("Enter a per-charge cap.");
      const { wallet, address } = await walletFor();
      const setup = await client.setupAutopay(sessionId, {
        code,
        max_amount: capBase,
        charges,
        interval_hours: hours,
        valid_days: days,
      });
      setCode("");
      await wallet.request({ method: "wallet_switchEthereumChain", params: [{ chainId: chainIdHex(setup.chain_id) }] });
      const signature = (await wallet.request({
        method: "eth_signTypedData_v4",
        params: [address, toSignTypedDataV4(setup.typed_data)],
      })) as string;
      await client.enableAutopay(sessionId, setup.mandate_id, setup.grant_id, signature);
      await refresh();
    });

  const revoke = (id: string) =>
    act(async () => {
      await client.revokeAutopay(sessionId, id);
      await refresh();
    });

  return (
    <section className="pay-stack">
      <h2>Autopay with {merchant || "this merchant"}</h2>
      <p className="pay-muted">
        Let the merchant charge you later without a code or wallet prompt. You approve once here with your authenticator
        code and one signature; every charge stays within these limits and you can revoke at any time.
      </p>
      {error ? (
        <p role="alert" className="pay-alert pay-alert-negative">
          {error}
        </p>
      ) : null}
      {mandates.length > 0 ? (
        <ul className="pay-stack">
          {mandates.map((m) => (
            <li key={m.mandate_id}>
              <span className={`pay-badge ${m.status === "active" ? "pay-badge-positive" : "pay-badge-notice"}`}>{m.status}</span>{" "}
              up to {show(m.max_amount)} per charge · {m.charges_remaining}/{m.charges} left · at most every{" "}
              {Math.round(m.interval_secs / 3600)} h · until {new Date(m.valid_until).toLocaleDateString()}{" "}
              {m.status === "active" ? (
                <button type="button" className="pay-btn pay-btn-tertiary" onClick={() => void revoke(m.mandate_id)} disabled={busy}>
                  Revoke
                </button>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
      <label className="pay-field">
        Per-charge cap
        <input value={cap || toDecimal(defaultCap)} onChange={(e) => setCap(e.target.value)} inputMode="decimal" />
      </label>
      <div className="pay-row">
        <label className="pay-field">
          Charges
          <input type="number" min={1} max={20} value={charges} onChange={(e) => setCharges(Number(e.target.value))} />
        </label>
        <label className="pay-field">
          Days
          <input type="number" min={1} max={30} value={days} onChange={(e) => setDays(Number(e.target.value))} />
        </label>
      </div>
      <label className="pay-field">
        Min hours apart
        <input type="number" min={0} max={744} value={hours} onChange={(e) => setHours(Number(e.target.value))} />
      </label>
      <p className={invalid ? "pay-alert pay-alert-negative" : "pay-muted"}>
        {invalid
          ? "Cap must be above 0; charges 1–20; hours 0–744; days 1–30."
          : `Most this can ever move: ${show((BigInt(capBase) * BigInt(charges)).toString())}.`}
      </p>
      <label className="pay-field">
        Authenticator code
        <input
          className="pay-otp"
          inputMode="numeric"
          autoComplete="one-time-code"
          maxLength={6}
          value={code}
          onChange={(e) => setCode(e.target.value.replace(/\D/g, "").slice(0, 6))}
        />
      </label>
      <div className="pay-actions">
        <button type="button" className="pay-btn" onClick={() => void setUp()} disabled={busy || invalid || code.length !== 6}>
          Set up autopay
        </button>
      </div>
    </section>
  );
}
