#!/bin/bash
# Deploy pcc-api (incl. private x402) to EC2 and keep mainnet gates open.
# Run in Terminal.app (Cursor cannot reach EC2 :22 from this network).
# Double-click this file, or:
#   open /Users/sudeepskamat/tachyonpe-checkout/scripts/deploy-ec2-api.command

set -euo pipefail
PEM="${PEM:-$HOME/Downloads/tachyon-mainet.pem}"
HOST="${PCC_EC2_HOST:-ubuntu@15.252.145.42}"
ROOT="${PCC_ROOT:-$HOME/tachyonpe-checkout}"
SSH=(ssh -i "$PEM" -o StrictHostKeyChecking=no -o ConnectTimeout=20)

echo "==> rsync backend → $HOST:/home/ubuntu/tachyon-PCC/backend/"
rsync -az --delete --exclude target -e "ssh -i $PEM -o StrictHostKeyChecking=no" \
  "$ROOT/backend/" "$HOST:/home/ubuntu/tachyon-PCC/backend/"

echo "==> release build + install + restart"
"${SSH[@]}" "$HOST" 'bash -s' <<'REMOTE'
set -euo pipefail
. "$HOME/.cargo/env"
export CARGO_TARGET_DIR=/tmp/pcc-target
export CARGO_INCREMENTAL=0
cd /home/ubuntu/tachyon-PCC/backend
cargo build --release -p pcc-api
strip -s /tmp/pcc-target/release/pcc-api || true
sudo cp /tmp/pcc-target/release/pcc-api /usr/local/bin/pcc-api
# Keep mainnet permanently open for live Harborline / x402 tests.
sudo sed -i 's/^DRY_RUN=.*/DRY_RUN=false/' /etc/pcc.env
sudo sed -i 's/^ENABLE_MAINNET_TRANSFERS=.*/ENABLE_MAINNET_TRANSFERS=true/' /etc/pcc.env
grep -E '^(DRY_RUN|ENABLE_MAINNET_TRANSFERS)=' /etc/pcc.env
sudo systemctl restart pcc-api
sleep 2
systemctl is-active pcc-api
curl -sS http://127.0.0.1:8080/health; echo
curl -sS http://127.0.0.1:8080/v1/meta/gates; echo
curl -sS http://127.0.0.1:8080/v1/x402/supported; echo
REMOTE

echo "==> public check"
curl -sS https://pay.tachyon.pe/v1/meta/gates; echo
curl -sS https://pay.tachyon.pe/v1/x402/supported; echo
echo "Done. Shop: https://harborline-amber.vercel.app"
read -r -p "Press enter to close…"
