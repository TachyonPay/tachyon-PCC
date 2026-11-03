import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  transpilePackages: ["@pcc/api-types", "@pcc/checkout-sdk", "@pcc/wallet-onboarding"],
  // No telemetry, no remote images, no experimental flags for a payment page.
  images: { remotePatterns: [] },
};

export default nextConfig;
