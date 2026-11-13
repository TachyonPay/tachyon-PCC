# Runbook: M4 session-key deposit (gated)

1Click settlement is **mainnet only**. M4 offline tests use a memory bundler.
Do not broadcast a session-key UserOp to a 1Click deposit address until the
owner approves that exact transaction in chat.

## Offline

```sh
cd backend && cargo fmt --all --check \
  && cargo clippy --workspace --all-targets --all-features -- -D warnings \
  && cargo test --workspace --all-features

cd services/integrations-ts && pnpm typecheck && pnpm test
```

## What the owner must approve before any live M4 send

1. Chain and Kernel implementation (`0xd6CEDDe84be40893d153Be9d467CD6aD37875b28`).
2. Session permission in plain language: token, max amount, expiry, rate-limit
   count, gas wei cap. Option C = any recipient on-chain; the service still
   binds the UserOp to the quote deposit address.
3. Source asset / destination asset / output amount / max debit / max fee.
4. Public owner and recipient addresses (no keys).
5. Permission to broadcast **one** previewed UserOp after preflight prints
   deposit address, `amountIn`, and estimated fee.

Until that approval, leave `DRY_RUN=true` and `ENABLE_MAINNET_TRANSFERS=false`.

## 7702 install (done)

Kernel v3.3 on Wallet A (Base): tx
[`0x3fa7c28e…e04d`](https://basescan.org/tx/0x3fa7c28e019abaccb6bed191dbd7363abce9f9d96fc9ea7d862945da2ab4e04d).
Code `0xef0100d6cedde84be40893d153be9d467cd6ad37875b28`. Session key
`0x06e6…50E7` holds no funds and needs none.

## Live 0.50 USDC session-key 1Click (done)

Owner-approved Base USDC → Arbitrum USDC, 0.50 out, session key (no second
owner signature).

* Deposit `0x2C724c6De0bfdB4A2291634852Cd2506E677d78B`
* Source UserOp tx [`0xbd8d23f7…119e`](https://basescan.org/tx/0xbd8d23f7f167752fc67de9b0b7b1a79b838f4b482e9ea4ad3315d4ae872a119e)
* Dest tx [`0xeae2b6f2…35ef`](https://arbiscan.io/tx/0xeae2b6f2bdc3ac06769bf8a8ff5536f400aac89e931602a79cafb675546535ef)
* 1Click `SUCCESS`; `amountIn` 0.505404, `amountOut` 0.50 USDC.
