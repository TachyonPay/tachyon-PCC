# Runbook: M3 Sepolia 7702 (gated)

1Click has no testnet. This runbook is **account behavior on Sepolia only**.
Default gates still refuse broadcast (`DRY_RUN=true`). Do not install a
delegation or send a UserOp until the owner has approved that exact step in
chat (implementation address, chain, token, amount).

## Offline

```sh
cd services/integrations-ts
pnpm test
```

## Read-only preflight

Needs `WALLET_A_ADDRESS` (the dedicated test EOA). Optional
`RPC_URL_SEPOLIA` (defaults to `https://ethereum-sepolia-rpc.publicnode.com`). Optional
`SESSION_SIGNER_KEY_REF` pointing at a **0600** file; if unset, the script
will create `secrets/session-signer.key` and print only the **address**.

```sh
cd services/integrations-ts
pnpm live:sepolia-preflight
```

Expected: JSON with `onboardDecision` (`install` if the EOA has no code),
session signer address, Kernel implementation
`0xd6CEDDe84be40893d153Be9d467CD6aD37875b28`, then **STOP**.

## Not in this runbook yet

Signing a 7702 authorization, installing Kernel, granting the session
permission, or sending a UserOp. Those need a funded Sepolia account
(ETH for the first authorization; ERC-20 for the transfer demo) and an
explicit owner approval of that transaction.
