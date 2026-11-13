#!/bin/sh
# Publish the hosted checkout (port 3000) on a temporary public HTTPS URL.
# The Next app proxies /v1 to the local API, so payers and merchant servers
# both use this one host. Restart pcc-api with the printed CHECKOUT_BASE_URL
# so new payment intents advertise that URL.
set -eu
if ! command -v cloudflared >/dev/null 2>&1; then
  echo "cloudflared is not installed. Install it, then run this again." >&2
  echo "https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/" >&2
  exit 1
fi
exec cloudflared tunnel --url http://127.0.0.1:3000
