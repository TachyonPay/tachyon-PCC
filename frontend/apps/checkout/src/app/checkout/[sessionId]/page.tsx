import { CheckoutPanel } from "./CheckoutPanel";

export const dynamic = "force-dynamic";

export default async function CheckoutSessionPage({
  params,
  searchParams,
}: {
  params: Promise<{ sessionId: string }>;
  searchParams: Promise<{ pcc_embed?: string }>;
}) {
  const { sessionId } = await params;
  const query = await searchParams;
  return <CheckoutPanel sessionId={sessionId} embed={query.pcc_embed === "1"} />;
}
