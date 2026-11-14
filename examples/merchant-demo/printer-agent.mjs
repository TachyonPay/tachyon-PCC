#!/usr/bin/env node
/**
 * Harborline printer agent. Run in Terminal.app (macOS Terminal), not Cursor.
 *
 * Spotlight: Terminal, then:
 *   cd examples/merchant-demo && node printer-agent.mjs
 *
 * Or double-click start-printer-agent.command
 *
 * This process holds the merchant key and talks to the A1 on LAN.
 * The public shop on Vercel never sees the printer.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { enqueueIfPaid, printerView, startWorker } from "./bambu.mjs";
import { merchantKey, pcc } from "./lib/pcc.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const dataDir = path.join(here, ".data");
const keyFile = path.join(dataDir, "merchant.key");
const jobsFile = path.join(dataDir, "print-jobs.json");

if (!process.env.MERCHANT_API_KEY) {
  try {
    process.env.MERCHANT_API_KEY = fs.readFileSync(keyFile, "utf8").trim();
  } catch {
    process.stderr.write("set MERCHANT_API_KEY or put it in .data/merchant.key (never commit it)\n");
    process.exit(2);
  }
}

process.env.BAMBU_AUTO_START = process.env.BAMBU_AUTO_START ?? "true";

/** @type {Map<string, object>} */
const jobs = new Map();

function loadJobs() {
  try {
    const parsed = JSON.parse(fs.readFileSync(jobsFile, "utf8"));
    const list = Array.isArray(parsed) ? parsed : [];
    for (const row of list) {
      if (row?.merchant_order_id) jobs.set(row.merchant_order_id, row);
    }
  } catch {
    // first run
  }
}

function persistJobs() {
  fs.mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  fs.writeFileSync(jobsFile, `${JSON.stringify([...jobs.values()], null, 2)}\n`, { mode: 0o600 });
}

function upsert(patch) {
  const key = patch.merchant_order_id;
  if (!key) return null;
  const next = { ...(jobs.get(key) ?? {}), ...patch, print: patch.print ? { ...(jobs.get(key)?.print ?? {}), ...patch.print } : jobs.get(key)?.print };
  jobs.set(key, next);
  persistJobs();
  return next;
}

function seedExisting(intents) {
  if (jobs.size > 0) return;
  for (const intent of intents) {
    if (intent.status !== "succeeded") continue;
    jobs.set(intent.merchant_order_id, {
      ...intent,
      print: { status: "skipped", error: "already on the books before this agent started" },
    });
  }
  if (jobs.size) persistJobs();
}

async function pull() {
  const key = merchantKey();
  const intents = await pcc("GET", "/v1/payment-intents", { key });
  const list = Array.isArray(intents) ? intents : [];
  seedExisting(list);
  for (const intent of list) {
    const prev = jobs.get(intent.merchant_order_id) ?? {};
    const row = { ...prev, ...intent, print: prev.print };
    jobs.set(intent.merchant_order_id, row);
    enqueueIfPaid(row, upsert);
  }
  persistJobs();
}

loadJobs();
startWorker(
  () => [...jobs.values()],
  upsert,
);

process.stderr.write("Harborline printer agent polling pay.tachyon.pe. Ctrl+C stops it. Keep this Terminal window open.\n");

await pull().catch((e) => {
  process.stderr.write(`poll failed: ${e.message}\n`);
});
const p = printerView();
process.stderr.write(`printer=${p.online ? "online" : "offline"} host=${p.host || "scanning"} auto_start=${p.auto_start}\n`);

setInterval(() => {
  void pull().catch((e) => process.stderr.write(`poll failed: ${e.message}\n`));
}, 8_000);
