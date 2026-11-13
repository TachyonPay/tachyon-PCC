import { createHmac } from "node:crypto";

export function base32Decode(s: string): Buffer {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = 0;
  let value = 0;
  const out: number[] = [];
  for (const c of s.replace(/=+$/, "").toUpperCase()) {
    value = (value << 5) | alphabet.indexOf(c);
    bits += 5;
    if (bits >= 8) {
      out.push((value >>> (bits - 8)) & 0xff);
      bits -= 8;
    }
  }
  return Buffer.from(out);
}

export function totp(secret: Buffer, now = Date.now()): string {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(now / 1000 / 30)));
  const h = createHmac("sha1", secret).update(counter).digest();
  const off = h[h.length - 1]! & 0x0f;
  const code = (h.readUInt32BE(off) & 0x7fffffff) % 1_000_000;
  return code.toString().padStart(6, "0");
}

/** The API accepts each code once per 30 s step; wait until a fresh one. */
export async function nextStep(last: number): Promise<number> {
  while (Math.floor(Date.now() / 30_000) === last) await new Promise((r) => setTimeout(r, 1_000));
  return Math.floor(Date.now() / 30_000);
}
