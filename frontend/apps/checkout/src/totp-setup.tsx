"use client";

import { useMemo, useState } from "react";
import { encode } from "./vendor/uqr/index.mjs";

export function totpSecretFromUri(uri: string): string {
  try {
    const parsed = new URL(uri);
    if (parsed.protocol !== "otpauth:") return "";
    return parsed.searchParams.get("secret") ?? "";
  } catch {
    return "";
  }
}

/** QR plus a selectable setup key. Never open otpauth:// on macOS; Passwords hijacks it. */
export function TotpSetup({ otpauth }: { otpauth: string }) {
  const secret = totpSecretFromUri(otpauth);
  const qr = useMemo(() => encode(otpauth, { ecc: "M" }), [otpauth]);
  const [copied, setCopied] = useState(false);
  const cells = qr.data as boolean[][];

  async function copyKey() {
    if (!secret) return;
    try {
      await navigator.clipboard.writeText(secret);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  }

  return (
    <div className="pay-totp-setup">
      <p className="pay-muted">Scan with Google Authenticator or Authy, or paste the setup key. macOS Passwords cannot save this code.</p>
      {cells.length ? (
        <div className="pay-totp-qr" role="img" aria-label="Authenticator QR code">
          <svg viewBox={`0 0 ${qr.size} ${qr.size}`} aria-hidden="true">
            {cells.map((row, y) =>
              row.map((on, x) => (on ? <rect key={`${x}-${y}`} x={x} y={y} width={1} height={1} /> : null)),
            )}
          </svg>
        </div>
      ) : null}
      {secret ? (
        <label className="pay-field">
          Setup key
          <input
            className="pay-wallet"
            readOnly
            value={secret}
            autoComplete="off"
            spellCheck={false}
            onFocus={(e) => e.currentTarget.select()}
          />
        </label>
      ) : null}
      {secret ? (
        <button type="button" className="pay-btn pay-btn-secondary" onClick={() => void copyKey()}>
          {copied ? "Copied" : "Copy setup key"}
        </button>
      ) : null}
    </div>
  );
}
