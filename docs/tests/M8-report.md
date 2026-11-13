# M8 — hardening / runbooks

Date: 2026-11-07. **No unattended mainnet.** No additional live send.

## What landed

* Runbooks: `docs/runbooks/m5-totp.md`, `m6-merchant-api.md`,
  `m7-hosted-checkout.md`, `m8-hardening.md`.
* Threat note: `docs/security/threat-notes-m5-m8.md`.
* README milestone table aligned to the build spec (M5 TOTP, M6 merchant
  API, M7 hosted checkout, M8 hardening). Previous README labels for M5–M7
  were swapped relative to the spec.
* Demo merchant seed is opt-in (`PCC_DEMO_MERCHANTS=true`) and keys are not
  logged.

## Explicitly out of scope this milestone

Re-installing EIP-7702 on Wallet A, any new Base/Arbitrum transfer, relaxing
`DRY_RUN` / `ENABLE_MAINNET_TRANSFERS`, or a cron/settlement worker that
broadcasts without a per-tx approval.
