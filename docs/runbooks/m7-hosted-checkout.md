# Runbook: M7 hosted checkout + browser SDK

## Hosted page

The hosted checkout is `CHECKOUT_BASE_URL/checkout/{session}`. The Next app
proxies `/v1` to the API, so a tunnel or public host in front of port 3000 is
enough for the payer. `checkout_url` uses `CHECKOUT_BASE_URL`; SIWE uses the
browser `Origin` when that origin is `http` or `https`.

* Public GET has no merchant API key.
* SIWE → dry 1Click preview → TOTP enroll → one-use authorize → executable
  quote + session simulate. Execute stays gated (`execution: "gated"`) while
  `DRY_RUN=true`. Requires `INTEGRATIONS_INTERNAL_TOKEN` on both `pcc-api` and
  the TS worker; without it authorize returns `execution: "unconfigured"`.
* Unknown session ids are 404 (unguessable UUID; stolen-id tests in `pcc-api`).
* Source asset: after SIWE, `GET /v1/checkout/{id}/source-assets` lists ERC-20s
  on Base/Arbitrum (never the merchant payout asset). `POST .../source-asset`
  re-prices before authorize; if that preview fails, `max_debit` is empty and
  authorize returns `409 preview_required`. Locked after authorize.
* Preflight (worker) checks token + ETH balance, bundler gas estimate, and that
  the payer already has the Kernel 7702 delegation on the source chain. The
  page can ask the wallet to install that delegation when the account has no
  code. A missing delegation still fails the payment with `simulation_failed`.
* Settlement: `pcc-api` polls 1Click status every `SETTLEMENT_POLL_SECS` for
  broadcast attempts. The worker verifies the status-embedded quote signature
  (restoring `quoteWaitingTimeMs`, which 1Click drops from the echo) and its
  deposit address. `settled` requires the Arbitrum/Base destination receipt to
  show the payout transfer to the merchant.
* `/v0/deposit/submit` is a best-effort hint, retried 3x in the background;
  1Click currently answers 500 `Empty AMQP response` and detects deposits anyway.
* Webhooks retry on the bounded backoff (1s doubling, cap 1h, 12 attempts);
  each due delivery is leased so retries never double-post.
* Live check: `pnpm --filter @pcc/integrations-ts e2e:checkout-gated`
  (`E2E_SOURCE_ASSET`, `E2E_EXPECT_EXECUTION` optional).

```sh
cd frontend && pnpm --filter @pcc/checkout dev   # :3000
```

## Browser SDK

`openHostedCheckout(url)` opens a popup with `noopener,noreferrer` only if
`isAllowedCheckoutUrl` accepts any `http` or `https` host. `javascript:`, `data:`, and URLs with credentials are still refused.
Rejected: `javascript:`, `data:`, credentialed URLs, public hosts.

Merchant methods on `CheckoutClient` require `apiKey` and must run on a
merchant backend (see `examples/merchant-demo`). The hosted page never
constructs a client with an API key.

`success_url` on intent create accepts a relative path or any `http`/`https`
host without credentials. `javascript:` and `data:` are still `open_redirect`.
The checkout page sends the payer's browser there with `pcc_result`
(`success`, `failure`, or `pending`), `pcc_intent`, `pcc_session`,
`pcc_status`, and when present `pcc_attempt_status` and `pcc_execution`.
The merchant server confirms the intent before fulfilling the order. The
checkout app proxies `/v1` to the API, so the public checkout host is also
the API host the payer's browser uses. Set `CHECKOUT_BASE_URL` to that public
origin so `checkout_url` is not `127.0.0.1`. A SIWE challenge uses the page
`Origin` when it is `http` or `https`.
