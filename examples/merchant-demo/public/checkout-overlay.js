/** Same overlay behaviour as `@pcc/checkout-sdk` `openHostedCheckout`. Merchant keys never enter this file. */
const MODAL_ID = "pcc-checkout-modal";
const MODAL = {
  overlay: "oklch(0% 0 0 / 0.72)",
  sheet: "oklch(5% 0 0)",
  closeBg: "oklch(15% 0 0)",
  border: "oklch(20% 0 0)",
  text: "oklch(95% 0 0)",
  radius: "8px",
  elevation: "0 24px 80px rgba(0, 0, 0, 0.45)",
  space5: "16px",
  space3: "8px",
  touch: "44px",
};

function isAllowedCheckoutUrl(url) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return false;
    if (parsed.username || parsed.password) return false;
    return parsed.hostname.length > 0;
  } catch {
    return false;
  }
}

function restoreBodyScroll(root) {
  if (!root) return;
  document.body.style.overflow = root.getAttribute("data-prev-overflow") ?? "";
}

export function closeHostedCheckout() {
  const root = document.getElementById(MODAL_ID);
  restoreBodyScroll(root);
  root?.remove();
}

export function openHostedCheckout(checkoutUrl) {
  if (!isAllowedCheckoutUrl(checkoutUrl)) throw new Error("checkout URL is not an allowed origin");
  closeHostedCheckout();
  const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const root = document.createElement("div");
  root.id = MODAL_ID;
  root.setAttribute("role", "dialog");
  root.setAttribute("aria-modal", "true");
  root.setAttribute("aria-label", "Checkout");
  root.setAttribute("data-prev-overflow", document.body.style.overflow);
  document.body.style.overflow = "hidden";
  root.style.cssText = `position:fixed;inset:0;z-index:2147483646;display:grid;place-items:center;padding:${MODAL.space5};background:${MODAL.overlay}`;
  const sheet = document.createElement("div");
  sheet.style.cssText = `position:relative;width:min(420px,100%);height:min(760px,92vh);background:${MODAL.sheet};border-radius:${MODAL.radius};overflow:hidden;box-shadow:${MODAL.elevation}`;
  const close = document.createElement("button");
  close.type = "button";
  close.setAttribute("aria-label", "Close checkout");
  close.textContent = "×";
  close.style.cssText = `position:absolute;top:${MODAL.space3};right:${MODAL.space3};z-index:1;width:${MODAL.touch};height:${MODAL.touch};padding:0;border:1px solid ${MODAL.border};border-radius:0;background:${MODAL.closeBg};color:${MODAL.text};cursor:pointer;font:500 20px/1 Inter,system-ui,sans-serif`;
  const frame = document.createElement("iframe");
  frame.title = "Tachyon checkout";
  const src = new URL(checkoutUrl);
  src.searchParams.set("pcc_embed", "1");
  frame.src = src.toString();
  frame.allow = "clipboard-write; publickey-credentials-get";
  frame.style.cssText = `width:100%;height:100%;border:0;background:${MODAL.sheet}`;
  const media = document.createElement("style");
  media.textContent = `@media (max-width: 480px){#${MODAL_ID}{padding:0!important;align-items:stretch!important}#${MODAL_ID}>div{width:100%!important;height:100%!important;max-height:none!important;border-radius:0!important}}`;
  const dismiss = () => {
    document.removeEventListener("keydown", onKey);
    window.removeEventListener("message", onMessage);
    closeHostedCheckout();
    previouslyFocused?.focus();
  };
  const onKey = (e) => {
    if (e.key === "Escape") dismiss();
  };
  const onMessage = (e) => {
    const data = e.data;
    if (data?.source === "pcc-checkout" && data.kind === "done") dismiss();
  };
  close.onclick = dismiss;
  root.addEventListener("click", (e) => {
    if (e.target === root) dismiss();
  });
  document.addEventListener("keydown", onKey);
  window.addEventListener("message", onMessage);
  sheet.append(close, frame);
  root.append(media, sheet);
  document.body.append(root);
  close.focus();
  return { close: dismiss };
}
