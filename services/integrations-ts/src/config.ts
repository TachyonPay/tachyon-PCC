/**
 * Worker configuration. Secrets are read from the environment and never
 * logged or echoed. `redactedSummary()` is the only thing safe to print.
 */
import { isIP } from "node:net";

export interface WorkerConfig {
  bindHost: string;
  bindPort: number;
  internalToken: string;
  oneClick: {
    baseUrl: string;
    apiKey: string | undefined;
    timeoutMs: number;
  };
}

export class ConfigError extends Error {}

function isPrivateHost(host: string): boolean {
  if (host === "localhost") return true;
  const v = isIP(host);
  if (v === 4) {
    return (
      host.startsWith("127.") ||
      host.startsWith("10.") ||
      host.startsWith("192.168.") ||
      /^172\.(1[6-9]|2\d|3[01])\./.test(host)
    );
  }
  if (v === 6) {
    const h = host.toLowerCase();
    return h === "::1" || h.startsWith("fc") || h.startsWith("fd") || h.startsWith("fe80");
  }
  return false;
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): WorkerConfig {
  const bindHost = env["INTEGRATIONS_BIND_HOST"] ?? "127.0.0.1";
  if (!isPrivateHost(bindHost)) {
    throw new ConfigError(`INTEGRATIONS_BIND_HOST=${bindHost} is not loopback/private; refusing to start`);
  }
  const bindPort = Number(env["INTEGRATIONS_BIND_PORT"] ?? "8787");
  if (!Number.isInteger(bindPort) || bindPort <= 0 || bindPort > 65535) {
    throw new ConfigError("INTEGRATIONS_BIND_PORT must be a valid port");
  }
  const internalToken = env["INTEGRATIONS_INTERNAL_TOKEN"] ?? "";
  if (internalToken.trim().length < 16) {
    throw new ConfigError("INTEGRATIONS_INTERNAL_TOKEN must be set (>=16 chars)");
  }
  const baseUrl = env["ONECLICK_BASE_URL"] ?? "https://1click.chaindefuser.com";
  if (!/^https:\/\//.test(baseUrl)) {
    throw new ConfigError("ONECLICK_BASE_URL must be https");
  }
  const apiKey = env["ONECLICK_API_KEY"]?.trim() || undefined;
  const timeoutMs = Number(env["ONECLICK_TIMEOUT_MS"] ?? "15000");

  return {
    bindHost,
    bindPort,
    internalToken,
    oneClick: { baseUrl, apiKey, timeoutMs },
  };
}

/** Safe to log. Never includes secret values. */
export function redactedSummary(cfg: WorkerConfig): Record<string, string | number | boolean> {
  return {
    bind: `${cfg.bindHost}:${cfg.bindPort}`,
    internalTokenConfigured: cfg.internalToken.length > 0,
    oneClickBaseUrl: cfg.oneClick.baseUrl,
    oneClickApiKeyConfigured: cfg.oneClick.apiKey !== undefined,
    oneClickTimeoutMs: cfg.oneClick.timeoutMs,
  };
}
