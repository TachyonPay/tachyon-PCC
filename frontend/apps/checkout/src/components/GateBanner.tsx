import { CheckoutClient } from "@pcc/checkout-sdk";

const API = process.env["NEXT_PUBLIC_API_BASE_URL"] ?? "http://127.0.0.1:8080";

/** Server component: reads execution gates from the Rust API. Fails soft. */
export async function GateBanner() {
  try {
    const gates = await new CheckoutClient({ baseUrl: API, timeoutMs: 2_000 }).gates();
    const live = !gates.dry_run && gates.enable_mainnet_transfers;
    return (
      <aside
        role="status"
        style={{
          border: `2px solid ${live ? "#b00020" : "#2e7d32"}`,
          padding: "0.75rem 1rem",
          borderRadius: 8,
          marginTop: "1.5rem",
        }}
      >
        <strong>{live ? "LIVE MODE — mainnet transfers enabled" : "Safe mode — dry run, no mainnet transfers"}</strong>
        <div style={{ fontSize: "0.9rem", marginTop: 4 }}>
          DRY_RUN={String(gates.dry_run)} · ENABLE_MAINNET_TRANSFERS={String(gates.enable_mainnet_transfers)}
        </div>
      </aside>
    );
  } catch {
    return (
      <aside role="status" style={{ border: "2px solid #999", padding: "0.75rem 1rem", borderRadius: 8, marginTop: "1.5rem" }}>
        API unreachable at <code>{API}</code> — start <code>pcc-api</code> to see gate status.
      </aside>
    );
  }
}
