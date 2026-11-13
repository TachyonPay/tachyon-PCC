# Harborline — example merchant shop

A storefront for a small 3D printed boat. Price is **0.20 USDC** on **Arbitrum**.
The payer sends on **Base**; Harborline receives USDC on **Arbitrum**.
The merchant API key stays in `server.mjs`. The browser only calls this shop,
then Tachyon hosted checkout.

```sh
# API + checkout + worker already running. From the repo root, with .env loaded
# (MERCHANT_SIGNUP_TOKEN). Do not export MERCHANT_API_KEY unless you want that
# merchant's name on checkout instead of Harborline.

node examples/merchant-demo/server.mjs      # http://127.0.0.1:3002
```

On boot the shop:

1. Signs up Harborline if there is no `MERCHANT_API_KEY` and `MERCHANT_SIGNUP_TOKEN` is set.
2. Writes payout to Circle USDC on Arbitrum (`WALLET_B_ADDRESS` if set).
3. Serves the product page. **Buy Mini Hull** `POST /api/checkout` creates a 0.20 USDC intent and opens hosted checkout as a modal.

For UI tests without WalletConnect, run Wallet A as a loopback signer (signatures only, no sends):

```sh
pnpm --filter @pcc/integrations-ts local:wallet   # 127.0.0.1:1790
```

Hosted checkout on localhost uses it instead of the QR. Keep `DRY_RUN=true` and `ENABLE_MAINNET_TRANSFERS=false` until the UI reaches the gated send. Then enable mainnet transfers in `.env` and restart the API and worker.

When a payment succeeds the shop queues a print on the Bambu A1 on this LAN
(MQTT 8883 + FTPS). Credentials stay in `.data/bambu.json`. Put a sliced
`.gcode.3mf` at `.data/mini-hull.gcode.3mf`. The printer is one job at a time;
later paid orders wait until the bed is idle. `BAMBU_HOST` overrides discovery.
`BAMBU_AUTO_START=false` queues without starting.

`POST /api/checkout` never returns the merchant key. `GET /api/order?intent=` confirms status with that key on the server.

`create-intent.mjs` is the one-shot CLI if you only need a checkout URL.
