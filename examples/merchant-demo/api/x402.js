import {
  createCheckout,
  x402Prepare,
  x402Requirements,
  x402Settle,
  x402Supported,
  x402Verify,
} from "../lib/pcc.mjs";

function readJson(req) {
  return new Promise((resolve, reject) => {
    if (req.body && typeof req.body === "object") {
      resolve(req.body);
      return;
    }
    let raw = "";
    req.on("data", (c) => {
      raw += c;
    });
    req.on("end", () => {
      if (!raw) {
        resolve({});
        return;
      }
      try {
        resolve(JSON.parse(raw));
      } catch (e) {
        reject(e);
      }
    });
    req.on("error", reject);
  });
}

export default async function handler(req, res) {
  res.setHeader("cache-control", "no-store");
  if (req.method === "OPTIONS") {
    res.status(204).end();
    return;
  }

  const q = req.query ?? {};
  const action = String(q.action || "create");

  try {
    if (action === "supported" && req.method === "GET") {
      res.status(200).json(await x402Supported());
      return;
    }
    if (action === "requirements" && req.method === "GET") {
      const session = String(q.session || "");
      if (!session) {
        res.status(400).json({ error: "session required" });
        return;
      }
      res.status(200).json(await x402Requirements(session));
      return;
    }
    if (action === "prepare" && req.method === "POST") {
      const session = String(q.session || "");
      if (!session) {
        res.status(400).json({ error: "session required" });
        return;
      }
      res.status(200).json(await x402Prepare(session, await readJson(req)));
      return;
    }
    if (action === "verify" && req.method === "POST") {
      res.status(200).json(await x402Verify(await readJson(req)));
      return;
    }
    if (action === "settle" && req.method === "POST") {
      res.status(200).json(await x402Settle(await readJson(req)));
      return;
    }
    if (action === "create" && req.method === "POST") {
      res.status(200).json(await createCheckout(req));
      return;
    }
    res.status(405).json({ error: "unsupported x402 action" });
  } catch (e) {
    res.status(e.status && e.status < 500 ? e.status : 502).json({ error: e.message, code: e.code });
  }
}
