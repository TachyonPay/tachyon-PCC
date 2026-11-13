# Tachyon UI

Colour and type are taken from [tachyon.pe](https://www.tachyon.pe/). Layout, spacing, and interaction follow Razorpay Blade Generative UI. Blade itself is not a dependency.

**Hosted checkout** is a 420px payment sheet. On a merchant site it opens as a modal overlay (`openHostedCheckout`). The same sheet is the full page at `/checkout/{session}` for a redirect. A popup is `{ mode: "popup" }` for wallets that refuse iframes.

**Merchant dashboard** is a Razorpay-style workspace: cream test-mode strip, grouped left nav, top bar with Collect payment, metric cards, and a payments table. Collect payment opens a modal. Wordmark and mark come from the Tachyon brand kit (`logo.svg`, cream `#FFFEF0` square + black glyph + lowercase `tachyon`).

## From tachyon.pe

| Role | Token | Value |
| --- | --- | --- |
| Surface | `--background` | `oklch(5% 0 0)` near-black |
| Card | `--card` | `oklch(8% 0 0)` |
| Text | `--foreground` | `oklch(95% 0 0)` |
| Primary (Launch app) | `--primary` | white on black |
| Brand cream (logo, test mode) | `--color-brand` | `#FFFEF0` |
| Border | `--border` | `oklch(20% 0 0)` |
| Body | Inter | |
| Headings / CTA | Space Grotesk | |
| Logo / mono | JetBrains Mono | |
| Corners | `0` on buttons, `8px` on cards | |

There is no gold on the marketing site. Checkout used `#F59E0B` on `#0F172A` as a placeholder before this mapping.

## Blade practices kept

- **Tokens, not raw values.** Spacing is Blade's 2px scale.
- **Primitives.** Card, Amount, Info group, Alert, Badge, Tabs, Modal, Button variants.
- **Tabs.** Dashboard sections are Tabs (distinct panels, arrow keys).
- **Modal.** Overlay, Escape, backdrop click, `aria-modal`, focus return, body scroll lock.
- **Accessibility.** Real `<button>`s, labelled fields, 44px touch, visible focus ring.
