"use client";

import type { ReactNode } from "react";
import { PrivyRoot } from "../privy";

export function Providers({ children }: { children: ReactNode }) {
  return <PrivyRoot>{children}</PrivyRoot>;
}
