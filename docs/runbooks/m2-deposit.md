# Runbook: M2 plain-EOA deposit (gated)

Default gates refuse every broadcast (`DRY_RUN=true`,
`ENABLE_MAINNET_TRANSFERS=false`). Do not flip them until the owner has
approved a specific transaction in chat.

## Offline / Anvil (no secrets)

```sh
cd backend
cargo test -p pcc-evm --all-features
```

Requires `anvil` on `PATH` (Foundry). Tests skip cleanly if it is missing;
CI installs Foundry.

## Dry preflight against 1Click (read-only)

Needs the TS worker running and `INTEGRATIONS_INTERNAL_TOKEN` in `.env`.
Optional public `WALLET_A_ADDRESS` / `WALLET_B_ADDRESS` (placeholders used
otherwise).

```sh
# terminal 1
cd services/integrations-ts && pnpm start

# terminal 2
cd backend && cargo run -p pcc-evm --bin pcc-deposit -- preflight
```

`--allocate-deposit` and `execute` are refused in this binary until M6
persistence **and** an explicit owner approval for one previewed tx.

## Live send (owner-approved only)

Not run as part of M2. When approved:

1. Dedicated test wallets A (source) and B (recipient), public addresses only.
2. Key for A in a `0600` file; `SOURCE_EXECUTOR_KEY_REF=/path/to/that/file`.
   Never put the key in `.env`.
3. `RPC_URL_BASE` = a reviewed Base RPC.
4. Print preflight (deposit address, amountIn, estimated fee).
5. Owner replies with an approval reference covering **that** destination,
   chain, token, amount, max fees.
6. Only then: `DRY_RUN=false` and `ENABLE_MAINNET_TRANSFERS=true` for that
   one process, then execute. Flip the gates back immediately after.
