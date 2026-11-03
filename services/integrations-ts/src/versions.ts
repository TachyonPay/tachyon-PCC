/** Report installed SDK versions (for /health and docs/sdk-compatibility.md). */
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

function pkgVersion(name: string): string {
  try {
    return (require(`${name}/package.json`) as { version: string }).version;
  } catch {
    return "unknown";
  }
}

export async function sdkVersions(): Promise<{ oneClickSdk: string; zerodevSdk: string }> {
  return {
    oneClickSdk: pkgVersion("@defuse-protocol/one-click-sdk-typescript"),
    zerodevSdk: pkgVersion("@zerodev/sdk"),
  };
}
