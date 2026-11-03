import { existsSync } from "node:fs";
import { resolve } from "node:path";

/** Load repo-root .env into process.env (Node >=21.7 built-in; no dotenv dep). */
export function loadRepoEnv(): void {
  const candidates = [resolve(process.cwd(), ".env"), resolve(process.cwd(), "../../.env")];
  for (const p of candidates) {
    if (existsSync(p)) {
      process.loadEnvFile(p);
      return;
    }
  }
}
