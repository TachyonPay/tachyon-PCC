import type { Metadata } from "next";
import type { ReactNode } from "react";

export const metadata: Metadata = { title: "PCC Merchant Dashboard" };

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body style={{ fontFamily: "system-ui, sans-serif", margin: 0, padding: "2rem", maxWidth: 960 }}>{children}</body>
    </html>
  );
}
