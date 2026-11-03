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

const server = createApp({ internalToken: cfg.internalToken, near, versions });
server.listen(cfg.bindPort, cfg.bindHost, () => {
  process.stderr.write(`pcc-integrations-ts listening ${JSON.stringify(redactedSummary(cfg))} sdk=${JSON.stringify(versions)}\n`);
});

for (const sig of ["SIGINT", "SIGTERM"] as const) {
  process.on(sig, () => {
    server.close(() => process.exit(0));
  });
}
