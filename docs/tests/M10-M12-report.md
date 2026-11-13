# M10–M12 — owner approval, payer flow, merchant onboarding

Date: 2026-11-07. **No transaction was signed or broadcast.** Gates stayed
closed (`DRY_RUN=true`, `ENABLE_MAINNET_TRANSFERS=false`) for every live run.
M9 (Postgres) is deferred; M13 (tight policy) is out of scope.

## M10 — owner-approval pause

`pcc-api` tests:

* `owner_mode_parks_deposit_until_operator_echoes_it`: authorize returns
  `awaiting_approval` and the executor is not called. A wrong token gets 401.
  Each mismatched field (amount, chain, max fee, deposit) gets 409 and the
  approval stays pending. The exact echo sends once, and a replay gets 404.
* `expired_or_rejected_approvals_never_send`: 410 after the window, 404 after
  a reject. The executor is never called.
* `payer_mode_sends_at_authorize`: `CHECKOUT_APPROVAL_MODE=payer` keeps the
  pre-M10 behaviour.
* `operator_routes_are_off_without_a_token`: 503 when no token is set.
* `approval_mode_defaults_to_payer`: unset, `payer` and `yes` send. Only
  exact `owner` (surrounding whitespace trimmed) parks the deposit.

Live smoke against the running API: the operator list returns 401 without the
token and 200 `[]` with it.

## M11 — payer flow

* `pcc-auth`: `configured_domain_is_exact`.
* `pcc-api`:
  * `siwe_uses_configured_domain_and_exact_message`
  * `siwe_domain_defaults_to_checkout_origin`
  * `authorize_requires_an_allowance_for_the_source_asset` (a rejected
    signature can be retried, a grant id enables once, re-pricing keeps the
    allowance)
  * `prepare_bounds_cap_uses_and_days`
  * `payer_can_see_and_revoke_the_allowance`
  * `each_sent_payment_spends_one_use` (2 uses: two sends, then 409; signed once)
  * `delegation_status_reports_each_source_chain`
* `pcc-integration-client`: `allowance_prepare_enable_status_revoke_round_trip`.
* `pcc-api`: `remembered_device_skips_wallet_sign_in_but_not_totp`,
  `device_tokens_expire_and_are_stored_hashed`,
  `a_second_wallet_never_inherits_the_first_wallets_attempt`.
* Worker: `grant_store.test.ts` (restart restores the session key and remaining
  uses; a short key, a wrong key and a modified file fail closed).
* Worker: `session_grants.test.ts` (12 tests: cap, uses, expiry and gas budget
  come from the request; out-of-range values are refused; the signature is
  checked against the owner; a grant id enables once; a new allowance replaces
  the old; uses are spent once per attempt; exhausted, expired and revoked
  allowances stop covering). Internal API contract tests cover prepare,
  enable, status and revoke.
* `@pcc/wallet-onboarding`: `toSignTypedDataV4` hashes identically to the
  server payload, and a signature over it recovers to the payer. Wallet-source
  selection and chain-id hex are also tested.
* `@pcc/checkout-sdk`: error codes surface; the allowance and delegation
  routes send no merchant key; allowance status maps `no_allowance` to null.

**Live gated E2E (Base USDC → Arbitrum USDC).** Wallet A signed the real
ZeroDev `Enable` typed data, the worker serialized and deserialized the
permission account, and the bundler preflight simulated:

```
{"phase":"siwe","quotedInput":"207419","maxDebit":"211568"}
{"phase":"grant","chainId":8453,"maxAmount":"211568"}
{"phase":"authorize","status":"authorized","execution":"gated","simulationOk":true}
```

The worker now starts with no wallet key loaded.

**TOTP is wallet-linked (spec M5).** Before this fix, sign-in created a new
credential on every checkout, so payers re-enrolled each time. Tests:

* `pcc-auth`: `confirmed_secret_is_never_replaced`.
* `pcc-api`: `totp_enrolls_once_per_wallet_and_serves_later_payments`. The
  second checkout reports `totp_enrolled: true`, re-enrollment returns 409, and
  a code alone authorizes. A different wallet still starts unenrolled.

Live, with `E2E_PAYMENTS=2` and Wallet A:

```
{"phase":"siwe","payment":1,"totpEnrolled":false,...}
{"phase":"enroll","payment":1}
{"phase":"authorize","payment":1,"execution":"gated","simulationOk":true,...}
{"phase":"siwe","payment":2,"totpEnrolled":true,...}
{"phase":"authorize","payment":2,"execution":"gated","simulationOk":true,...}
```

**Standing allowance (sign once, then TOTP only).** Live, 2026-11-08, with
`E2E_PAYMENTS=2`, Wallet A, Base USDC, gates closed:

```
{"phase":"allowance","payment":1,"reused":false,"chainId":8453,"maxAmount":"423136","uses":2,"totalExposure":"846272"}
{"phase":"enroll","payment":1}
{"phase":"authorize","payment":1,"execution":"gated","simulationOk":true,...}
{"phase":"siwe","payment":2,"totpEnrolled":true,...}
{"phase":"allowance","payment":2,"reused":true,"usesRemaining":2}
{"phase":"authorize","payment":2,"execution":"gated","simulationOk":true,...}
{"phase":"summary","payments":2,"allowanceSignatures":1}
```

Both payments simulated through the same session key. Gated payments spent no
use. Nothing was sent.

## M12 — merchant onboarding

`pcc-api` tests:

* `invite_gated_signup_issues_working_credentials`
* `signup_is_off_without_a_token`
* `rotating_the_api_key_revokes_the_old_one`
* `rotating_the_webhook_secret_changes_signatures`

`@pcc/checkout-sdk` tests: signup sends `X-Signup-Token` and no bearer key;
rotation and payout readback require a key.

Live smoke results:

* A bad invite returns 401; a valid one returns 201.
* The key and secret formats match.
* Payout readback returns 200.
* After rotation the old key gets 401 on `/v1/me` and the new key gets 200.
* Webhook secret rotation returns 200.

## Autopay mandates

`pcc-api` tests:

* `autopay_setup_needs_enrollment_a_fresh_code_and_one_signature`:
  * An unenrolled wallet gets 409 and a wrong code gets 401.
  * Out-of-range charges, interval or days get 400.
  * An unsigned mandate is neither listed nor chargeable.
  * A wrong grant id gets 409.
  * The mandate does not stand in for the checkout's own allowance.
* `merchant_charges_a_mandate_without_the_payer`:
  * A charge with no key gets 401; an over-cap charge gets 409.
  * Two charges send in payer mode, and each plan carries the mandate id
    and an `autopay:` approval ref.
  * An idempotent replay returns the first result without charging again.
  * A third charge gets `mandate_exhausted`.
* `mandate_interval_and_payer_revoke_stop_charges`:
  * Owner mode parks the charge for approval, and an early second charge
    gets 429.
  * Revoke destroys the worker key, and later charges get
    `mandate_inactive`.
  * Another wallet cannot revoke.

Elsewhere:

* `pcc-integration-client`: the allowance round trip carries `mandateId`.
* Worker: a mandate allowance sits beside the checkout allowance; spending
  and revoking one leaves the other alone. Contract tests cover `mandateId`
  on prepare, enable and status, and reject a malformed one.
* `@pcc/checkout-sdk`: payer autopay routes send no key; merchant routes send
  the key and an idempotency key.

**Live gated E2E (`pnpm e2e:autopay-gated`), 2026-11-08,** Wallet A, Base
USDC → Arbitrum USDC, gates closed:

```
{"phase":"enroll","maxDebit":"211568"}
{"phase":"mandate","status":"active","chainId":8453,"maxAmount":"423136","charges":2,"totalExposure":"846272"}
{"phase":"charge","charge":1,"execution":"gated","simulationOk":true,"quotedInput":"207419",...,"chargesRemaining":1}
{"phase":"charge","charge":2,"execution":"gated","simulationOk":true,"quotedInput":"207419",...,"chargesRemaining":0}
{"phase":"over_limit","status":409,"code":"mandate_exhausted"}
{"phase":"revoke","status":"revoked",...}
```

The payer made two signatures (SIWE and the mandate), neither a transaction,
and one TOTP code for setup. The merchant charged twice with its API key
only. Both charges got real executable 1Click quotes and passed the bundler
preflight through the mandate's own session key. Nothing was sent.

The first two attempts failed with 502 `quote_failed` while 1Click's
`/v0/quote` was timing out for every caller, including the standalone
`live:dry-quote` probe, for about 10 minutes. Those failed charges were not
counted.

## Browser checks

These used the in-IDE browser, the public Alice demo key, and deliberately
wrong tokens only:

* **Dashboard:**
  * The merchant loads.
  * The payout reads back into the form.
  * Intents are listed.
  * Signup with a wrong invite shows the API's 401.
  * This caught a missing `X-Signup-Token` in the CORS allow-list, now fixed and
    covered by `dashboard_preflight_allows_signup_and_operator_headers`.
* **`/operator`:** a wrong token shows "401 unauthorized: operator token
  required".
* **Checkout:** the order renders. With no wallet installed, the page says so
  and names the WalletConnect setting.

## Totals

| Suite | Result |
| --- | --- |
| Rust workspace (`clippy -D warnings`, `fmt --check`) | 165 pass, 1 ignored (network); `pcc-api` 39 |
| Worker (`integrations-ts`) | 120 pass |
| `@pcc/checkout-sdk` | 13 pass |
| `@pcc/wallet-onboarding` | 13 pass |
| Frontend typecheck + `next build` (checkout, dashboard) | pass |
