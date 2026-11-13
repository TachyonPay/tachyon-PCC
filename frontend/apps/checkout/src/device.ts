/**
 * Remembered-device token in this browser. It replaces only the wallet
 * sign-in: every payment still needs the payer's TOTP code.
 */
const KEY = "pcc.device.v1";

export interface RememberedDevice {
  token: string;
  address: string;
}

export function loadDevice(): RememberedDevice | null {
  try {
    const raw = window.localStorage.getItem(KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return null;
    const { token, address } = parsed as Record<string, unknown>;
    if (typeof token !== "string" || !token.startsWith("pcc_dev_") || typeof address !== "string") return null;
    return { token, address };
  } catch {
    return null;
  }
}

export function saveDevice(device: RememberedDevice): void {
  try {
    window.localStorage.setItem(KEY, JSON.stringify(device));
  } catch {
    // Storage disabled (private mode): the payer signs in with the wallet next time.
  }
}

export function clearDevice(): void {
  try {
    window.localStorage.removeItem(KEY);
  } catch {
    // Nothing stored.
  }
}
