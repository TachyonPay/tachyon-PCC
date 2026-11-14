"use client";

import { useEffect, useState } from "react";

type Accept = {
  scheme: string;
  network: string;
  max_amount_required: string;
  asset: string;
  pay_to: string;
  extra: {
    chain_id: number;
    chain_name: string;
    asset_id: string;
    decimals: number;
    symbol: string;
  };
};

type Prepare = {
  x402_version: number;
  attempt_id: string;
  quoted_input: string;
  deposit_address: string;
  chain_id: number;
  chain_name: string;
  token_address: string;
  decimals: number;
  symbol: string;
  accepts: Accept;
  dry_run: boolean;
};

type Eip1193 = {
  request: (args: { method: string; params?: unknown[] }) => Promise<unknown>;
};

function units(base: string, decimals: number): string {
  if (!/^[0-9]+$/.test(base)) return base;
  const padded = base.padStart(decimals + 1, "0");
  const whole = padded.slice(0, -decimals) || "0";
  const frac = padded.slice(-decimals).replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : whole;
}

function encodeTransfer(to: string, amount: string): string {
  const addr = to.replace(/^0x/i, "").toLowerCase().padStart(64, "0");
  const amt = BigInt(amount).toString(16).padStart(64, "0");
  return `0xa9059cbb${addr}${amt}`;
}

async function post<T>(path: string, body?: unknown): Promise<T> {
  const init: RequestInit = body
    ? {
        method: "POST",
        headers: { accept: "application/json", "content-type": "application/json" },
        body: JSON.stringify(body),
      }
    : { method: "GET", headers: { accept: "application/json" } };
  const res = await fetch(path, init);
  const json = (await res.json().catch(() => ({}))) as { error?: string };
  if (!res.ok) throw new Error(json.error || "x402 request failed");
  return json as T;
}

export function X402Pay(props: {
  sessionId: string;
  busy: boolean;
  onBusy: (v: boolean) => void;
  onError: (msg: string) => void;
  onPaid: (execution: string, intentStatus: string) => void;
  onBack: () => void;
}) {
  const [accepts, setAccepts] = useState<Accept[]>([]);
  const [picked, setPicked] = useState<Accept | null>(null);
  const [payer, setPayer] = useState("");
  const [dryRun, setDryRun] = useState(false);
  const [status, setStatus] = useState("Loading pay-in options…");

  useEffect(() => {
    let cancelled = false;
    void post<{ accepts: Accept[]; dry_run: boolean }>(`/v1/x402/sessions/${props.sessionId}/requirements`)
      .then((r) => {
        if (cancelled) return;
        const rows = [...(r.accepts ?? [])].sort((a, b) => {
          const rank = (s: string) => (s === "USDC" ? 0 : s === "USDT" || s === "USDT0" ? 1 : 2);
          return rank(a.extra.symbol) - rank(b.extra.symbol) || a.extra.chain_name.localeCompare(b.extra.chain_name);
        });
        setAccepts(rows);
        setPicked(rows.find((a) => a.extra.symbol === "USDC") ?? rows[0] ?? null);
        setDryRun(Boolean(r.dry_run));
        setStatus("Pick a chain and asset, then pay from your wallet.");
      })
      .catch((e: unknown) => {
        if (!cancelled) props.onError(e instanceof Error ? e.message : "Could not load x402 options");
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- load once per session
  }, [props.sessionId]);

  const wallet = (): Eip1193 => {
    const eth = (window as unknown as { ethereum?: Eip1193 }).ethereum;
    if (!eth) throw new Error("No injected wallet. Open MetaMask or Rabby.");
    return eth;
  };

  const connect = async () => {
    props.onError("");
    const accounts = (await wallet().request({ method: "eth_requestAccounts" })) as string[];
    const addr = accounts?.[0] ?? "";
    if (!addr) throw new Error("Wallet returned no account");
    setPayer(addr);
  };

  const pay = async (simulate: boolean) => {
    if (!picked) throw new Error("Pick a chain first");
    if (!payer) throw new Error("Connect a wallet first");
    props.onBusy(true);
    props.onError("");
    try {
      setStatus("Preparing confidential deposit…");
      const prep = await post<Prepare>(`/v1/x402/sessions/${props.sessionId}/prepare`, {
        asset_id: picked.extra.asset_id,
        payer,
      });
      let txHash: string;
      if (simulate) {
        if (!prep.dry_run) throw new Error("Dry-run is off. Send the transfer from your wallet.");
        txHash = `dryrun:checkout-${Date.now()}`;
      } else {
        const eth = wallet();
        await eth.request({
          method: "wallet_switchEthereumChain",
          params: [{ chainId: `0x${prep.chain_id.toString(16)}` }],
        });
        setStatus(`Confirm ${units(prep.quoted_input, prep.decimals)} ${prep.symbol} in your wallet…`);
        txHash = String(
          await eth.request({
            method: "eth_sendTransaction",
            params: [
              {
                from: payer,
                to: prep.token_address,
                data: encodeTransfer(prep.deposit_address, prep.quoted_input),
                value: "0x0",
              },
            ],
          }),
        );
      }
      const body = {
        payment_payload: {
          x402_version: prep.x402_version,
          scheme: prep.accepts.scheme,
          network: prep.accepts.network,
          payload: {
            tx_hash: txHash,
            from: payer,
            amount: prep.quoted_input,
            asset: prep.accepts.asset,
            pay_to: prep.deposit_address,
            attempt_id: prep.attempt_id,
          },
        },
        payment_requirements: prep.accepts,
      };
      setStatus("Verifying…");
      const ver = await post<{ is_valid: boolean; invalid_reason?: string }>("/v1/x402/verify", body);
      if (!ver.is_valid) throw new Error(ver.invalid_reason || "Payment rejected");
      setStatus("Settling…");
      const set = await post<{ success: boolean; error?: string; execution?: string; intent_status?: string }>(
        "/v1/x402/settle",
        body,
      );
      if (!set.success) throw new Error(set.error || "Settle failed");
      props.onPaid(set.execution || "x402_wallet", set.intent_status || "processing");
    } finally {
      props.onBusy(false);
    }
  };

  return (
    <section className="pay-stack">
      <p className="pay-muted">
        One wallet transfer. No authenticator and no delegation. Harborline still receives USDC on Arbitrum.
      </p>
      <label className="pay-field">
        Pay with
        <select
          value={picked?.extra.asset_id ?? ""}
          disabled={props.busy || accepts.length === 0}
          onChange={(e) => setPicked(accepts.find((a) => a.extra.asset_id === e.target.value) ?? null)}
        >
          {accepts.map((a) => (
            <option key={a.extra.asset_id} value={a.extra.asset_id}>
              {a.extra.symbol} on {a.extra.chain_name} · ~{units(a.max_amount_required, a.extra.decimals)}
            </option>
          ))}
        </select>
      </label>
      <p className="pay-muted">{payer ? payer : status}</p>
      <div className="pay-actions">
        <button type="button" className="pay-btn" disabled={props.busy} onClick={() => void connect().catch((e: unknown) => props.onError(e instanceof Error ? e.message : "Connect failed"))}>
          {payer ? "Wallet connected" : "Connect wallet"}
        </button>
        <button
          type="button"
          className="pay-btn"
          disabled={props.busy || !payer || !picked}
          onClick={() => void pay(false).catch((e: unknown) => props.onError(e instanceof Error ? e.message : "Payment failed"))}
        >
          {props.busy ? "Waiting…" : "Pay from wallet"}
        </button>
        {dryRun ? (
          <button
            type="button"
            className="pay-btn pay-btn-secondary"
            disabled={props.busy || !payer || !picked}
            onClick={() => void pay(true).catch((e: unknown) => props.onError(e instanceof Error ? e.message : "Simulate failed"))}
          >
            Simulate (dry run)
          </button>
        ) : null}
        <button type="button" className="pay-btn pay-btn-tertiary" disabled={props.busy} onClick={props.onBack}>
          Use private checkout instead
        </button>
      </div>
    </section>
  );
}
