# Runbook: live read-only 1Click checks

These are the only scripts that contact a third party with a credential. Both
are read-only and cannot create an executable quote or move funds.

## Preconditions

1. `ONECLICK_API_KEY` set in the repo-root `.env` (never committed). The agent
   and scripts only check *presence*, never print the value.
2. `ONECLICK_BASE_URL=https://1click.chaindefuser.com` (non-https is refused).
3. Node 24 + pnpm 10, `pnpm install --frozen-lockfile` in `services/integrations-ts`.

## Commands

```sh
cd services/integrations-ts
pnpm live:tokens      # GET /v0/tokens → EVM chain/token matrix (no key strictly required, sent if present)
pnpm live:dry-quote   # POST /v0/quote dry:true, EXACT_OUTPUT, confidentiality:basic, verifies signature
```

Default dry quote: Base USDC → Arbitrum USDC, `amountOut = 1000000` (1 USDC),
placeholder recipient/refund addresses (`0x111…`, `0x222…`). Override with
`ORIGIN_ASSET`, `DESTINATION_ASSET`, `AMOUNT_OUT`, `RECIPIENT`, `REFUND_TO`.

## Expected output

* Exit 0, a redacted quote summary: assets, `amountIn`/`minAmountIn`/
  `amountOut`, `signatureVerified: true`, `confidentiality: basic`,
  `dry: true`, no `depositAddress` (dry quotes have none).
* Exit 2 if the key is missing. Non-zero with a contract error code
  (`UPSTREAM_UNAUTHORIZED`, `NO_ROUTE`, `UPSTREAM_RATE_LIMITED`, …) otherwise.

## Record keeping

Paste the redacted output (never headers) into `docs/tests/` for the milestone.
