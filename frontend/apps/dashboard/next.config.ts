import type { NextConfig } from "next";

const api = process.env["API_PROXY_TARGET"] ?? "http://127.0.0.1:8080";

const nextConfig: NextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  transpilePackages: ["@pcc/api-types", "@pcc/checkout-sdk"],
  images: { remotePatterns: [] },
  async rewrites() {
    return [
      { source: "/v1/:path*", destination: `${api}/v1/:path*` },
      { source: "/health", destination: `${api}/health` },
    ];
  },
  allowedDevOrigins: ["127.0.0.1", "localhost", "*.trycloudflare.com"],
};

export default nextConfig;
