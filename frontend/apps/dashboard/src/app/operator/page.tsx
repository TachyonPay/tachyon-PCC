import { CheckoutClient } from "@pcc/checkout-sdk";
import { DashboardShell } from "@/components/DashboardShell";
import { OperatorPanel } from "@/components/OperatorPanel";

const API = process.env.API_PROXY_TARGET ?? "http://127.0.0.1:8080";

export const metadata = { title: "Tachyon Operator" };

export default async function OperatorPage() {
  let testMode = true;
  try {
    const gates = await new CheckoutClient({ baseUrl: API, timeoutMs: 2_000 }).gates();
    testMode = gates.dry_run || !gates.enable_mainnet_transfers;
  } catch {
    /* fail soft */
  }
  return (
    <DashboardShell
      nav={[
        { id: "pay", label: "Payments", href: "/", group: "Collect", icon: "pay" },
        { id: "operator", label: "Operator", href: "/operator", icon: "operator" },
      ]}
      current="operator"
      title="Operator"
      testMode={testMode}
    >
      <p className="dash-lead">
        Issue merchant invites here. Payments send when the payer authorizes. Set <code>CHECKOUT_APPROVAL_MODE=owner</code> to
        park them for review.
      </p>
      <OperatorPanel />
    </DashboardShell>
  );
}
