import { PAYOUT } from "../lib/pcc.mjs";

export default async function handler(req, res) {
  res.setHeader("cache-control", "no-store");
  if (req.method !== "GET") {
    res.status(405).json({ error: "GET only" });
    return;
  }
  res.status(200).json({ merchant: "Harborline", ...PAYOUT });
}
