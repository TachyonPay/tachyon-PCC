/**
 * Load SESSION_SIGNER_KEY_REF from a 0600 file. Raw 64-hex in the env var is
 * refused (same rule as Rust SOURCE_EXECUTOR_KEY_REF). Key bytes are never
 * logged or included in error messages.
 */
import { readFileSync, statSync } from "node:fs";
import { privateKeyToAccount, type PrivateKeyAccount } from "viem/accounts";
import type { Hex } from "viem";

export class KeyRefError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "KeyRefError";
  }
}

function looksLikeRawKey(s: string): boolean {
  const t = s.trim();
  const hex = t.startsWith("0x") || t.startsWith("0X") ? t.slice(2) : t;
  return hex.length === 64 && /^[0-9a-fA-F]+$/.test(hex) && !t.includes("/") && !t.includes(".");
}

function asPath(keyRef: string): string {
  const t = keyRef.trim();
  return t.startsWith("file:") ? t.slice("file:".length) : t;
}

export function loadSessionSigner(keyRef: string): PrivateKeyAccount {
  const t = keyRef.trim();
  if (!t) throw new KeyRefError("SESSION_SIGNER_KEY_REF is empty; no signer loaded");
  if (looksLikeRawKey(t)) {
    throw new KeyRefError("SESSION_SIGNER_KEY_REF looks like a raw private key; put it in a 0600 file");
  }
  return loadSessionSignerFromPath(asPath(t));
}

export function loadSessionSignerFromPath(path: string): PrivateKeyAccount {
  let mode: number;
  try {
    mode = statSync(path).mode & 0o777;
  } catch {
    throw new KeyRefError("SESSION_SIGNER_KEY_REF file is unreadable");
  }
  if ((mode & 0o077) !== 0) {
    throw new KeyRefError(`SESSION_SIGNER_KEY_REF file is not 0600 (mode ${mode.toString(8)}); refusing to load`);
  }
  let text: string;
  try {
    text = readFileSync(path, "utf8");
  } catch {
    throw new KeyRefError("SESSION_SIGNER_KEY_REF file is unreadable");
  }
  const line = text
    .split("\n")
    .map((l) => l.trim())
    .find((l) => l.length > 0 && !l.startsWith("#"));
  if (!line) throw new KeyRefError("SESSION_SIGNER_KEY_REF file did not contain a usable key");
  const hex = (line.startsWith("0x") || line.startsWith("0X") ? line : `0x${line}`) as Hex;
  try {
    return privateKeyToAccount(hex);
  } catch {
    throw new KeyRefError("SESSION_SIGNER_KEY_REF file did not contain a usable key");
  }
}
