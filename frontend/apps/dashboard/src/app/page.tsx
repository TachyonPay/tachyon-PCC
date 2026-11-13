import { CheckoutClient } from "@pcc/checkout-sdk";
import { MerchantPanel } from "@/components/MerchantPanel";

const API = "";

export const dynamic = "force-dynamic";

export default async function DashboardHome() {
  let testMode = true;
  try {
    const gates = await new CheckoutClient({ baseUrl: API || "http://127.0.0.1:8080", timeoutMs: 2_000 }).gates();
    testMode = gates.dry_run || !gates.enable_mainnet_transfers;
  } catch {
    /* fail soft — assume test mode */
  }
  return <MerchantPanel testMode={testMode} />;
}
