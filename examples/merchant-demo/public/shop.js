import { openHostedCheckout } from "./checkout-overlay.js";

const buy = document.getElementById("buy");
const status = document.getElementById("status");
const errorBox = document.getElementById("error");
const orderList = document.getElementById("order-list");
const printerStatus = document.getElementById("printer-status");

function showError(msg) {
  if (!errorBox) return;
  errorBox.hidden = !msg;
  errorBox.textContent = msg ?? "";
}

function units(v) {
  if (!/^[0-9]+$/.test(v || "")) return v || "—";
  const padded = v.padStart(7, "0");
  const frac = padded.slice(-6).replace(/0+$/, "");
  return frac ? `${padded.slice(0, -6)}.${frac}` : padded.slice(0, -6);
}

function statusLabel(s) {
  if (s === "succeeded") return "Placed";
  if (s === "requires_payment") return "Open";
  return (s || "pending").replaceAll("_", " ");
}

function printLabel(p) {
  if (!p?.status) return "";
  if (p.status === "queued") return p.error ? `Queued on Bambu (${p.error})` : "Queued on Bambu A1";
  if (p.status === "sending") return "Sending to Bambu…";
  if (p.status === "printing") return "Printing on Bambu A1";
  if (p.status === "done") return "Print finished";
  if (p.status === "error") return p.error || "Print failed";
  return p.status;
}

function renderOrders(orders) {
  if (!orderList) return;
  if (!orders.length) {
    orderList.innerHTML = `<p class="muted">No Mini Hull orders yet.</p>`;
    return;
  }
  orderList.innerHTML = orders
    .map((o) => {
      const received = `${units(o.output_amount)} USDC on ${o.settle_chain}`;
      const paid = o.quoted_input ? `${units(o.quoted_input)} USDC on ${o.pay_chain}` : `Pay on ${o.pay_chain}`;
      const href = `/return.html?order=${encodeURIComponent(o.merchant_order_id)}&pcc_result=${o.status === "succeeded" ? "success" : "pending"}`;
      return `<article class="order-card">
        <p class="ok-badge">${statusLabel(o.status)}</p>
        <h3>${o.product || "Mini Hull"}</h3>
        <p class="mono">${o.merchant_order_id}</p>
        <p>${paid} → Harborline received ${received}</p>
        ${o.print?.status ? `<p class="print-line">${printLabel(o.print)}</p>` : ""}
        <p class="actions">
          <a href="${href}">Order details</a>
          ${o.explorers?.base ? `<a href="${o.explorers.base}" rel="noreferrer">BaseScan</a>` : ""}
          ${o.explorers?.arbitrum ? `<a href="${o.explorers.arbitrum}" rel="noreferrer">Arbiscan</a>` : ""}
          <a href="${o.explorers?.confidential}">No correlation</a>
        </p>
      </article>`;
    })
    .join("");
}

const printerCam = document.getElementById("printer-cam");
const printerCamCaption = document.getElementById("printer-cam-caption");

function printerLine(p) {
  if (!p.configured) return "Printer credentials missing.";
  if (!p.deps) return "Printer bridge needs npm install in the shop folder.";
  if (!p.file_ready) return "No sliced Mini Hull file yet. Drop a .gcode.3mf in the shop data folder.";
  if (!p.online) {
    return p.error
      ? `Bambu A1 offline. ${p.error}. Paid orders still queue.`
      : "Bambu A1 offline. Paid orders still queue until it is on this LAN.";
  }
  const state = (p.gcode_state || "idle").toLowerCase();
  const heat = p.nozzle != null && p.bed != null ? ` Nozzle ${p.nozzle}°C, bed ${p.bed}°C.` : "";
  const layer = p.layer != null && p.layers ? ` Layer ${p.layer}/${p.layers}.` : "";
  const pct = p.percent != null ? ` ${p.percent}% done.` : "";
  return `Bambu A1 ${state}.${heat}${layer}${pct} New paid Mini Hulls go into the queue.`;
}

function showCamera(online) {
  if (!printerCam) return;
  if (!online) {
    printerCam.hidden = true;
    if (printerCamCaption) printerCamCaption.textContent = "Camera waits until the A1 is on this LAN.";
    return;
  }
  printerCam.hidden = false;
  printerCam.src = `/api/printer/camera.jpg?t=${Date.now()}`;
  if (printerCamCaption) printerCamCaption.textContent = "Live chamber camera on the A1.";
}

async function loadPrinter() {
  if (!printerStatus) return;
  try {
    const res = await fetch("/api/printer", { headers: { accept: "application/json" } });
    const p = await res.json();
    printerStatus.textContent = printerLine(p);
    showCamera(Boolean(p.online));
  } catch {
    printerStatus.textContent = "Could not read printer status.";
  }
}

async function loadOrders() {
  if (!orderList) return;
  try {
    const res = await fetch("/api/orders", { headers: { accept: "application/json" } });
    const body = await res.json();
    renderOrders(body.orders ?? []);
  } catch {
    orderList.innerHTML = `<p class="muted">Orders will show here after a successful checkout.</p>`;
  }
}

function goToReturn(data) {
  const u = new URL("/return.html", location.origin);
  if (data.intentId) u.searchParams.set("pcc_intent", data.intentId);
  if (data.sessionId) u.searchParams.set("pcc_session", data.sessionId);
  u.searchParams.set("pcc_result", data.result || "pending");
  if (data.status) u.searchParams.set("pcc_status", data.status);
  location.assign(u);
}

window.addEventListener("message", (e) => {
  const data = e.data;
  if (data?.source !== "pcc-checkout" || data.kind !== "done") return;
  goToReturn(data);
});

buy?.addEventListener("click", async () => {
  showError("");
  buy.disabled = true;
  if (status) status.textContent = "Opening checkout…";
  try {
    const res = await fetch("/api/checkout", { method: "POST", headers: { accept: "application/json" } });
    const body = await res.json();
    if (!res.ok) throw new Error(body.error ?? "Could not create the order");
    if (status) status.textContent = `Order ${body.order_id}`;
    openHostedCheckout(body.checkout_url);
  } catch (e) {
    showError(e instanceof Error ? e.message : "Checkout failed");
    if (status) status.textContent = "";
  } finally {
    buy.disabled = false;
  }
});

void loadOrders();
void loadPrinter();
setInterval(() => {
  void loadOrders();
  void loadPrinter();
}, 5000);
