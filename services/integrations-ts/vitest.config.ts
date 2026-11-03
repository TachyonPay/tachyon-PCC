import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["tests/**/*.test.ts"],
    environment: "node",
    testTimeout: 10_000,
    // No network in unit tests. Live scripts live under scripts/ and are run explicitly.
    env: {
      ONECLICK_API_KEY: "",
      INTEGRATIONS_INTERNAL_TOKEN: "test-internal-token",
    },
  },
});
