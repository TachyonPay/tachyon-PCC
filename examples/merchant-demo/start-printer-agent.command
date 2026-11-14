#!/bin/zsh
cd "$(dirname "$0")"
export API_BASE_URL="${API_BASE_URL:-https://pay.tachyon.pe}"
export BAMBU_AUTO_START="${BAMBU_AUTO_START:-true}"
echo "Harborline printer agent. Leave this Terminal window open."
exec node printer-agent.mjs
