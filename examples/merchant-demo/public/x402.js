const q = new URLSearchParams(location.search);
const session = q.get("session") || "";
const intent = q.get("intent") || "";
const order = q.get("order") || "";

const orderLine = document.getElementById("order-line");
const gateLine = document.getElementById("gate-line");
const assetList = document.getElementById("asset-list");
const walletLine = document.getElementById("wallet-line");
const connectBtn = document.getElementById("connect");
const payBtn = document.getElementById("pay");
const dryBtn = document.getElementById("dry");
const statusEl = document.getElementById("status");
const errorBox = document.getElementById("error");

/** @type {any} */
let requirements = null;
/** @type {any} */
let selected = null;
/** @type {any} */
let prepared = null;
let payer = "";
let dryRun = true;

function showError(msg) {
  errorBox.hidden = !msg;
  errorBox.textContent = msg ?? "";
}

function setStatus(msg) {
  statusEl.textContent = msg ?? "";
}

function units(v, decimals = 6) {
  if (!/^[0-9]+$/.test(v || "")) return v || "—";
  const padded = String(v).padStart(decimals + 1, "0");
  const whole = padded.slice(0, -decimals) || "0";
  const frac = padded.slice(-decimals).replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : whole;
}

function chainIdHex(id) {
  return `0x${Number(id).toString(16)}`;
}

function encodeTransfer(to, amountBase) {
  const sel = "a9059cbb";
  const addr = to.replace(/^0x/i, "").toLowerCase().padStart(64, "0");
  const amt = BigInt(amountBase).toString(16).padStart(64, "0");
  return `0x${sel}${addr}${amt}`;
}

async function api(action, { method = "GET", sessionId = "", body } = {}) {
  const u = new URL("/api/x402", location.origin);
  u.searchParams.set("action", action);
  if (sessionId) u.searchParams.set("session", sessionId);
  const res = await fetch(u, {
    method,
    headers: {
      accept: "application/json",
      ...(body ? { "content-type": "application/json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(json.error || `x402 ${action} failed`);
  return json;
}

function ethereum() {
  const eth = window.ethereum;
  if (!eth) throw new Error("No injected wallet found. Install MetaMask or Rabby.");
  return eth;
}

function renderAssets() {
  const accepts = requirements?.accepts ?? [];
  if (!accepts.length) {
    assetList.innerHTML = `<p class="muted">No pay-in assets available right now.</p>`;
    return;
  }
  assetList.innerHTML = accepts
    .map((a, i) => {
      const ex = a.extra || {};
      const label = `${ex.symbol || "USDC"} on ${ex.chain_name || a.network}`;
      const amount = units(a.max_amount_required, ex.decimals ?? 6);
      const active = selected === a ? " is-selected" : "";
      return `<button type="button" class="asset-card${active}" data-i="${i}" role="option" aria-selected="${selected === a}">
        <span class="asset-title">${label}</span>
        <span class="mono">You send ~${amount} ${ex.symbol || "USDC"}</span>
      </button>`;
    })
    .join("");
  assetList.querySelectorAll(".asset-card").forEach((btn) => {
    btn.addEventListener("click", () => {
      selected = accepts[Number(btn.getAttribute("data-i"))];
      prepared = null;
      payBtn.disabled = !payer;
      dryBtn.disabled = !payer;
      renderAssets();
      setStatus(`Selected ${selected.extra?.chain_name || ""} ${selected.extra?.symbol || ""}`);
    });
  });
}

async function connect() {
  showError("");
  const eth = ethereum();
  const accounts = await eth.request({ method: "eth_requestAccounts" });
  payer = String(accounts[0] || "");
  if (!payer) throw new Error("Wallet returned no account");
  walletLine.textContent = `Connected ${payer}`;
  connectBtn.textContent = "Wallet connected";
  payBtn.disabled = !selected;
  dryBtn.disabled = !selected || !dryRun;
}

async function preparePayment() {
  if (!selected) throw new Error("Pick a chain and asset first");
  if (!payer) throw new Error("Connect a wallet first");
  setStatus("Preparing confidential deposit…");
  prepared = await api("prepare", {
    method: "POST",
    sessionId: session,
    body: { asset_id: selected.extra.asset_id, payer },
  });
  selected = prepared.accepts;
  return prepared;
}

function paymentBody(txHash) {
  return {
    payment_payload: {
      x402_version: prepared.x402_version || 1,
      scheme: prepared.accepts.scheme,
      network: prepared.accepts.network,
      payload: {
        tx_hash: txHash,
        from: payer,
        amount: prepared.quoted_input,
        asset: prepared.accepts.asset,
        pay_to: prepared.deposit_address,
        attempt_id: prepared.attempt_id,
      },
    },
    payment_requirements: prepared.accepts,
  };
}

async function settleWith(txHash) {
  const body = paymentBody(txHash);
  setStatus("Verifying payment…");
  const ver = await api("verify", { method: "POST", body });
  if (!ver.is_valid) throw new Error(ver.invalid_reason || "Payment rejected");
  setStatus("Settling via facilitator…");
  const set = await api("settle", { method: "POST", body });
  if (!set.success) throw new Error(set.error || "Settle failed");
  const u = new URL("/return.html", location.origin);
  u.searchParams.set("pcc_intent", intent || set.intent_id || "");
  u.searchParams.set("pcc_session", session);
  u.searchParams.set("order", order);
  u.searchParams.set("pcc_result", set.intent_status === "succeeded" ? "success" : "pending");
  u.searchParams.set("pcc_status", set.intent_status || "processing");
  location.assign(u);
}

async function payLive() {
  showError("");
  payBtn.disabled = true;
  dryBtn.disabled = true;
  try {
    const prep = await preparePayment();
    const eth = ethereum();
    const chainId = prep.chain_id;
    try {
      await eth.request({
        method: "wallet_switchEthereumChain",
        params: [{ chainId: chainIdHex(chainId) }],
      });
    } catch (e) {
      throw new Error(`Switch wallet to ${prep.chain_name} (chain ${chainId}) and try again`);
    }
    setStatus(`Confirm ${units(prep.quoted_input)} ${prep.symbol} transfer in your wallet…`);
    const txHash = await eth.request({
      method: "eth_sendTransaction",
      params: [
        {
          from: payer,
          to: prep.token_address,
          data: encodeTransfer(prep.deposit_address, prep.quoted_input),
          value: "0x0",
        },
      ],
    });
    await settleWith(txHash);
  } catch (e) {
    showError(e instanceof Error ? e.message : "Payment failed");
    setStatus("");
  } finally {
    payBtn.disabled = false;
    dryBtn.disabled = !dryRun;
  }
}

async function payDry() {
  showError("");
  payBtn.disabled = true;
  dryBtn.disabled = true;
  try {
    if (!dryRun) throw new Error("Dry-run simulate is off while mainnet gates are open");
    await preparePayment();
    await settleWith(`dryrun:harborline-${Date.now()}`);
  } catch (e) {
    showError(e instanceof Error ? e.message : "Dry-run failed");
    setStatus("");
  } finally {
    payBtn.disabled = false;
    dryBtn.disabled = false;
  }
}

async function boot() {
  if (!session) {
    showError("Missing checkout session. Start again from the shop.");
    return;
  }
  orderLine.textContent = `Order ${order || "—"} · intent ${intent || "—"}`;
  try {
    requirements = await api("requirements", { sessionId: session });
    dryRun = Boolean(requirements.dry_run);
    if (dryRun) {
      gateLine.hidden = false;
      gateLine.textContent =
        "API gates are in dry-run. You can simulate settlement without a mainnet transfer, or still send a real wallet transfer for testing.";
      dryBtn.hidden = false;
    } else {
      gateLine.hidden = false;
      gateLine.textContent = "Live settlement is enabled. Send USDC from your wallet to the confidential deposit.";
    }
    connectBtn.disabled = false;
    renderAssets();
    setStatus("Pick a pay-in chain, then connect your wallet.");
  } catch (e) {
    showError(
      e instanceof Error
        ? `${e.message}. If this is local, run pcc-api with the new x402 routes before using this page.`
        : "Could not load x402 requirements",
    );
  }
}

connectBtn?.addEventListener("click", () => {
  void connect().catch((e) => showError(e instanceof Error ? e.message : "Connect failed"));
});
payBtn?.addEventListener("click", () => void payLive());
dryBtn?.addEventListener("click", () => void payDry());

void boot();
