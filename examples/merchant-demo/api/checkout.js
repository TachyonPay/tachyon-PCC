import { createCheckout } from "../lib/pcc.mjs";

export default async function handler(req, res) {
  res.setHeader("cache-control", "no-store");
  if (req.method === "OPTIONS") {
    res.status(204).end();
    return;
  }
  if (req.method !== "POST") {
    res.status(405).json({ error: "POST only" });
    return;
  }
  try {
    res.status(200).json(await createCheckout(req));
  } catch (e) {
    res.status(e.status && e.status < 500 ? e.status : 502).json({ error: e.message, code: e.code });
  }
}
