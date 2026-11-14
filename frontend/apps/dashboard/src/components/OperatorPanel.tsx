"use client";

import { useCallback, useRef, useState } from "react";

const API = "";

const CHAIN_NAMES: Record<number, string> = { 1: "Ethereum", 10: "Optimism", 8453: "Base", 42161: "Arbitrum" };

/** Mirrors `pcc_api::operator::PendingApprovalView` (operator routes are not in the public OpenAPI). */
interface PendingApproval {
  attempt_id: string;
  intent_id: string;
  chain_id: number;
  token: string;
  from: string;
  deposit_address: string;
  amount: string;
  max_fee_wei: string;
  expires_at: string;
}

interface ApproveResult {
  attempt_id: string;
  execution: string;
  source_tx_hash: string | null;
  attempt_status: string;
}

async function operatorCall<T>(token: string, method: string, path: string, body?: unknown): Promise<T | null> {
  const res = await fetch(`${API}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      accept: "application/json",
      ...(body ? { "content-type": "application/json" } : {}),
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const text = await res.text();
  const json = text ? (JSON.parse(text) as unknown) : null;
  if (!res.ok) {
    const err = json as { code?: string; error?: string } | null;
    throw new Error(`${res.status} ${err?.code ?? ""}: ${err?.error ?? "request failed"}`);
  }
  return json as T | null;
}

export function OperatorPanel() {
  const tokenRef = useRef<HTMLInputElement>(null);
  const [pending, setPending] = useState<PendingApproval[] | null>(null);
  const [checked, setChecked] = useState<Record<string, boolean>>({});
  const [result, setResult] = useState<string | null>(null);
  const [invite, setInvite] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = useCallback(async (body: (token: string) => Promise<void>) => {
    const token = tokenRef.current?.value.trim() ?? "";
    if (!token) {
      setError("Enter the operator token.");
      return;
    }
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      await body(token);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Operator API unreachable");
    } finally {
      setBusy(false);
    }
  }, []);

  const refresh = useCallback(
    () =>
      run(async (token) => {
        setPending((await operatorCall<PendingApproval[]>(token, "GET", "/v1/operator/approvals")) ?? []);
        setChecked({});
      }),
    [run],
  );

  const approve = useCallback(
    (p: PendingApproval) =>
      run(async (token) => {
        const out = await operatorCall<ApproveResult>(token, "POST", `/v1/operator/approvals/${p.attempt_id}/approve`, {
          chain_id: p.chain_id,
          deposit_address: p.deposit_address,
          amount: p.amount,
          max_fee_wei: p.max_fee_wei,
        });
        setResult(
          `Approved ${p.attempt_id}: execution ${out?.execution ?? "?"}, status ${out?.attempt_status ?? "?"}` +
            (out?.source_tx_hash ? `, tx ${out.source_tx_hash}` : ""),
        );
        setPending((list) => list?.filter((x) => x.attempt_id !== p.attempt_id) ?? null);
      }),
    [run],
  );

  const reject = useCallback(
    (p: PendingApproval) =>
      run(async (token) => {
        await operatorCall<null>(token, "POST", `/v1/operator/approvals/${p.attempt_id}/reject`);
        setResult(`Rejected ${p.attempt_id}. Nothing was sent.`);
        setPending((list) => list?.filter((x) => x.attempt_id !== p.attempt_id) ?? null);
      }),
    [run],
  );

  return (
    <section>
      <div className="dash-card">
        <label className="dash-field">
          Operator token
          <input ref={tokenRef} type="password" autoComplete="off" defaultValue="" />
        </label>
        <div className="dash-card-footer">
          <button type="button" className="dash-btn dash-btn-secondary" onClick={() => void refresh()} disabled={busy}>
            Load pending approvals
          </button>
          <button
            type="button"
            className="dash-btn"
            disabled={busy}
            onClick={() =>
              void run(async (token) => {
                const out = await operatorCall<{ invite_token: string }>(token, "POST", "/v1/operator/invites");
                setInvite(out?.invite_token ?? null);
                setResult("Invite issued. It works once. Copy it now; this page does not store it.");
              })
            }
          >
            Issue one-time merchant invite
          </button>
        </div>
      </div>
      {invite ? (
        <div className="dash-secret" role="status">
          Invite <code>{invite}</code>
        </div>
      ) : null}
      {error ? (
        <p role="alert" className="dash-alert">
          {error}
        </p>
      ) : null}
      {result ? (
        <p role="status" className="dash-ok">
          {result}
        </p>
      ) : null}
      {pending && pending.length === 0 ? <p className="dash-muted">Nothing is waiting for approval.</p> : null}
      {pending?.map((p) => (
        <article key={p.attempt_id} className="dash-card">
          <dl className="dash-info">
            <dt>Chain</dt>
            <dd>
              {CHAIN_NAMES[p.chain_id] ?? "unknown"} ({p.chain_id})
            </dd>
            <dt>Token</dt>
            <dd>
              <code>{p.token}</code>
            </dd>
            <dt>From (payer)</dt>
            <dd>
              <code>{p.from}</code>
            </dd>
            <dt>To (1Click deposit)</dt>
            <dd>
              <code>{p.deposit_address}</code>
            </dd>
            <dt>Amount (base units)</dt>
            <dd>
              <code>{p.amount}</code>
            </dd>
            <dt>Max fee (wei)</dt>
            <dd>
              <code>{p.max_fee_wei}</code>
            </dd>
            <dt>Expires</dt>
            <dd>{new Date(p.expires_at).toLocaleString()}</dd>
          </dl>
          <label className="dash-check">
            <input
              type="checkbox"
              checked={checked[p.attempt_id] ?? false}
              onChange={(e) => setChecked((c) => ({ ...c, [p.attempt_id]: e.target.checked }))}
            />
            <span>I checked the chain, token, deposit address, amount and max fee above. Approving sends this one transfer.</span>
          </label>
          <div className="dash-card-footer">
            <button type="button" className="dash-btn" onClick={() => void approve(p)} disabled={busy || !checked[p.attempt_id]}>
              Approve this exact transfer
            </button>
            <button type="button" className="dash-btn dash-btn-secondary" onClick={() => void reject(p)} disabled={busy}>
              Reject
            </button>
          </div>
        </article>
      ))}
    </section>
  );
}
