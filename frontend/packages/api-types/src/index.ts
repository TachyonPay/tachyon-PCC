// Public API types. `public-v1.d.ts` is GENERATED from the Rust OpenAPI
// document (`pnpm gen`); never hand-edit it. Amounts are JSON strings.
import type { components, paths } from "./public-v1.js";

export type { components, paths };

export type Health = components["schemas"]["Health"];
export type GatesView = components["schemas"]["GatesView"];
export type PaymentIntentView = components["schemas"]["PaymentIntentView"];
export type CheckoutView = components["schemas"]["CheckoutView"];
export type MerchantView = components["schemas"]["MerchantView"];
export type PayoutRequest = components["schemas"]["PayoutRequest"];
export type PayoutView = components["schemas"]["PayoutView"];
export type SignupResponse = components["schemas"]["SignupResponse"];
export type ApiKeyResponse = components["schemas"]["ApiKeyResponse"];
export type WebhookSecretResponse = components["schemas"]["WebhookSecretResponse"];
export type SessionGrantView = components["schemas"]["SessionGrantView"];
export type PrepareGrantRequest = components["schemas"]["PrepareGrantRequest"];
export type AllowanceView = components["schemas"]["AllowanceView"];
export type RevokeResponse = components["schemas"]["RevokeResponse"];
export type MandateSetupRequest = components["schemas"]["MandateSetupRequest"];
export type MandateSetupView = components["schemas"]["MandateSetupView"];
export type MandateView = components["schemas"]["MandateView"];
export type ChargeRequest = components["schemas"]["ChargeRequest"];
export type ChargeView = components["schemas"]["ChargeView"];
export type SiweVerifyResponse = components["schemas"]["SiweVerifyResponse"];
export type AuthorizeResponse = components["schemas"]["AuthorizeResponse"];
export type ForgetResponse = components["schemas"]["ForgetResponse"];
export type DelegationStatusResponse = components["schemas"]["DelegationStatusResponse"];
export type ChainDelegationView = components["schemas"]["ChainDelegationView"];

/** Base path for the public API; the Rust server binds to loopback by default. */
export const PUBLIC_API_VERSION = "0.1.0" as const;
