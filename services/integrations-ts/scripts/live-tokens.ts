/**
 * LIVE read-only call: GET /v0/tokens. Prints the EVM route-availability
 * matrix. Requires ONECLICK_API_KEY in the repo-root .env (loaded here,
 * never printed). Moves no funds; allocates no deposit address.
 */
import { loadRepoEnv } from "./_env.js";
import { OneClickClient } from "../src/near/client.js";

loadRepoEnv();
const apiKey = process.env["ONECLICK_API_KEY"]?.trim();
if (!apiKey) {
  console.error("ONECLICK_API_KEY not set in .env — refusing to call 1Click.");
  process.exit(2);
}

const EVM_SLUGS = new Map([
  ["eth", 1], ["op", 10], ["bsc", 56], ["gnosis", 100], ["pol", 137],
  ["base", 8453], ["arb", 42161], ["avax", 43114], ["scroll", 534352],
]);

const client = new OneClickClient({ baseUrl: process.env["ONECLICK_BASE_URL"] ?? "https://1click.chaindefuser.com", apiKey, timeoutMs: 20_000 });
const tokens = await client.getTokens();
const evm = tokens.filter((t) => EVM_SLUGS.has(t.blockchain));
console.log(`1Click tokens: total=${tokens.length} evm_mapped=${evm.length}`);
const byChain = new Map<string, typeof evm>();
for (const t of evm) byChain.set(t.blockchain, [...(byChain.get(t.blockchain) ?? []), t]);
for (const [chain, list] of [...byChain.entries()].sort()) {
  console.log(`\n${chain} (chainId ${EVM_SLUGS.get(chain)}): ${list.length} assets`);
  for (const t of list.sort((a, b) => a.symbol.localeCompare(b.symbol))) {
    console.log(`  ${t.symbol.padEnd(8)} dec=${String(t.decimals).padEnd(3)} ${t.assetId}`);
  }
}
