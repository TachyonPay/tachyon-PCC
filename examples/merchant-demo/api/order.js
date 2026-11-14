import { getShopOrder } from "../lib/pcc.mjs";

export default async function handler(req, res) {
  res.setHeader("cache-control", "no-store");
  if (req.method !== "GET") {
    res.status(405).json({ error: "GET only" });
    return;
  }
  try {
    const q = req.query ?? {};
    const row = await getShopOrder({
      intent: q.intent || "",
      order: q.order || "",
      session: q.session || "",
    });
    if (!row) {
      res.status(404).json({ error: "order not found" });
      return;
    }
    res.status(200).json(row);
  } catch (e) {
    res.status(e.status && e.status < 500 ? e.status : 502).json({ error: e.message, code: e.code });
  }
}
