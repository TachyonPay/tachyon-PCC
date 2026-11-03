// Public API types. `public-v1.d.ts` is GENERATED from the Rust OpenAPI
// document (`pnpm gen`); never hand-edit it. Amounts are JSON strings.
import type { components, paths } from "./public-v1.js";

export type { components, paths };

export type Health = components["schemas"]["Health"];
export type GatesView = components["schemas"]["GatesView"];

/** Base path for the public API; the Rust server binds to loopback by default. */
export const PUBLIC_API_VERSION = "0.1.0" as const;
