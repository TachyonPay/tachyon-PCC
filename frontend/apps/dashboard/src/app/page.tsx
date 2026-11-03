import { CheckoutClient } from "@pcc/checkout-sdk";

const API = process.env["NEXT_PUBLIC_API_BASE_URL"] ?? "http://127.0.0.1:8080";

// Gate status must reflect the running API, never a build-time snapshot.
export const dynamic = "force-dynamic";

export default async function DashboardHome() {
  let status = "unreachable";
  try {
    const client = new CheckoutClient({ baseUrl: API, timeoutMs: 2_000 });
    const [h, g] = await Promise.all([client.health(), client.gates()]);
    status = `${h.service} v${h.version} · dry_run=${g.dry_run} · mainnet_transfers=${g.enable_mainnet_transfers}`;
  } catch {
    /* fail soft: dashboard shell must render without the API */
  }
  return (
    <main>
      <h1>Merchant dashboard</h1>
      <p>Shell only (M0/M1). Payout configuration, API keys, payment intents and settlement views land in M6/M7.</p>
      <p>
        API: <code>{API}</code> — {status}
      </p>
    </main>
  );
}
