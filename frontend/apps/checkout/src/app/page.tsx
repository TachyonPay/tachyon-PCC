import { GateBanner } from "@/components/GateBanner";

// Gate status must reflect the running API, never a build-time snapshot.
export const dynamic = "force-dynamic";

export default function CheckoutHome() {
  return (
    <main>
      <h1>Private Crypto Checkout</h1>
      <p>
        Checkout shell (M0/M1). The payer flow — wallet connect, EIP-7702 onboarding, verified quote display and
        bounded authorisation — is added in M3/M4. Nothing on this page can move funds.
      </p>
      <GateBanner />
    </main>
  );
}
