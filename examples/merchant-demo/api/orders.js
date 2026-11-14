import { listShopOrders } from "../lib/pcc.mjs";

export default async function handler(req, res) {
  res.setHeader("cache-control", "no-store");
  if (req.method !== "GET") {
    res.status(405).json({ error: "GET only" });
    return;
  }
  try {
    res.status(200).json({ orders: await listShopOrders() });
  } catch (e) {
    res.status(e.status && e.status < 500 ? e.status : 502).json({ error: e.message, code: e.code });
  }
}
