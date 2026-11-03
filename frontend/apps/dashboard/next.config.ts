import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  transpilePackages: ["@pcc/api-types", "@pcc/checkout-sdk"],
  images: { remotePatterns: [] },
};

export default nextConfig;
