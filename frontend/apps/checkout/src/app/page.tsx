import { BrandMark } from "../components/BrandMark";

export const dynamic = "force-dynamic";

export default function CheckoutHome() {
  return (
    <main className="pay-home">
      <BrandMark />
      <h1>Pay</h1>
      <p className="pay-muted">Open the checkout link from your merchant. This page cannot take a payment on its own.</p>
    </main>
  );
}
