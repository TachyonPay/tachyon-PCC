import { mkdirSync } from "node:fs";
import { AaService } from "../account-abstraction/service.js";
import { SessionKeyExecutor, type BundlerPort } from "../account-abstraction/executor.js";
import { MemoryBundler } from "../account-abstraction/bundler.js";
import { KernelBundler } from "../account-abstraction/kernel_bundler.js";
import { KernelGrantBackend } from "../account-abstraction/kernel_grants.js";
import { EncryptedGrantStore } from "../account-abstraction/grant_store.js";
import { SessionGrantService } from "../account-abstraction/session_grants.js";
import { SessionLedger } from "../account-abstraction/ledger.js";
import { gatesFromEnv } from "../account-abstraction/gates.js";
import { loadConfig, redactedSummary } from "../config.js";
import { OneClickClient } from "../near/client.js";
import { NearService } from "../near/service.js";
import { sdkVersions } from "../versions.js";
import { createApp } from "./app.js";

const cfg = loadConfig();
const client = new OneClickClient({
  baseUrl: cfg.oneClick.baseUrl,
  apiKey: cfg.oneClick.apiKey,
  timeoutMs: cfg.oneClick.timeoutMs,
});
const near = new NearService(client);
const versions = await sdkVersions();
const ledgerDir = process.env["SESSION_LEDGER_DIR"] ?? ".pcc-aa-ledger";
mkdirSync(ledgerDir, { recursive: true });
const gates = gatesFromEnv();

// Payers sign their own session grants; this process holds no wallet keys.
// Session keys are encrypted at rest when GRANT_STORE_KEY is set; otherwise a restart drops every allowance.
const storeKey = process.env["GRANT_STORE_KEY"]?.trim();
const grantStore = storeKey
  ? new EncryptedGrantStore(process.env["GRANT_STORE_DIR"]?.trim() || ".pcc-grant-store", storeKey)
  : undefined;
const grants = new SessionGrantService(KernelGrantBackend.fromEnv(), grantStore ? { store: grantStore } : {});
process.stderr.write(
  grantStore
    ? `pcc-integrations-ts allowance store: encrypted, ${grants.size} active restored\n`
    : "pcc-integrations-ts allowance store: memory only (set GRANT_STORE_KEY to keep allowances across restarts)\n",
);
const bundler: BundlerPort =
  process.env["AA_BUNDLER"]?.trim() === "memory" ? new MemoryBundler() : KernelBundler.fromEnv(grants);

const executor = new SessionKeyExecutor(gates, new SessionLedger(ledgerDir), bundler);
const aa = new AaService(executor, bundler, grants);

const server = createApp({ internalToken: cfg.internalToken, near, versions, aa });
server.listen(cfg.bindPort, cfg.bindHost, () => {
  process.stderr.write(`pcc-integrations-ts listening ${JSON.stringify(redactedSummary(cfg))} sdk=${JSON.stringify(versions)}\n`);
});

for (const sig of ["SIGINT", "SIGTERM"] as const) {
  process.on(sig, () => {
    server.close(() => process.exit(0));
  });
}
