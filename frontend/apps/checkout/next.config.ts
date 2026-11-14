import type { NextConfig } from "next";

const api = process.env["API_PROXY_TARGET"] ?? "http://127.0.0.1:8080";

const nextConfig: NextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  transpilePackages: ["@pcc/api-types", "@pcc/checkout-sdk", "@pcc/wallet-onboarding", "@zerodev/sdk"],
  // No telemetry, no remote images, no experimental flags for a payment page.
  images: { remotePatterns: [] },
  // The payer's browser talks to this origin. Next forwards `/v1` to the API,
  // so one public checkout host is enough for both the page and the API.
  async rewrites() {
    return [
      { source: "/v1/:path*", destination: `${api}/v1/:path*` },
      { source: "/health", destination: `${api}/health` },
    ];
  },
  allowedDevOrigins: ["127.0.0.1", "localhost", "*.trycloudflare.com"],
};

export default nextConfig;
