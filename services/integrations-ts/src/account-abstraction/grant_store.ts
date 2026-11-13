/**
 * Encrypted at-rest store for active allowances, so a worker restart keeps
 * payers' session keys instead of asking them to sign again.
 *
 * One file, rewritten atomically on every change: AES-256-GCM under a key
 * derived (HKDF-SHA256, fresh salt per write) from `GRANT_STORE_KEY`. A wrong
 * key or a modified file fails closed: load throws rather than starting empty
 * and overwriting allowances that are still installed on chain.
 */
import { createCipheriv, createDecipheriv, hkdfSync, randomBytes } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Address, Hex } from "viem";
import type { SessionGrant } from "./validate.js";

export const MIN_STORE_KEY_LENGTH = 32;
const FILE = "grants.v1.enc";
const INFO = "pcc.grant-store.v1";

/** JSON form of an active allowance: bigints as decimal strings, sets as arrays. */
export interface PersistedGrant {
  grant: Omit<SessionGrant, "maxAmount" | "maxGas" | "maxGasWei"> & {
    maxAmount: string;
    maxGas: string;
    maxGasWei: string;
  };
  mandateId?: string;
  sessionKey: Hex;
  serialized: string;
  usesRemaining: number;
  consumedAttempts: string[];
}

export interface GrantStore {
  load(): PersistedGrant[];
  save(grants: PersistedGrant[]): void;
}

export class GrantStoreError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "GrantStoreError";
  }
}

interface Envelope {
  v: 1;
  salt: string;
  iv: string;
  tag: string;
  ct: string;
}

export class EncryptedGrantStore implements GrantStore {
  private readonly file: string;
  private readonly secret: Buffer;

  constructor(dir: string, secret: string) {
    if (secret.length < MIN_STORE_KEY_LENGTH) {
      throw new GrantStoreError(`GRANT_STORE_KEY must be at least ${MIN_STORE_KEY_LENGTH} characters`);
    }
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    this.file = join(dir, FILE);
    this.secret = Buffer.from(secret, "utf8");
  }

  load(): PersistedGrant[] {
    if (!existsSync(this.file)) return [];
    let env: Envelope;
    try {
      env = JSON.parse(readFileSync(this.file, "utf8")) as Envelope;
    } catch {
      throw new GrantStoreError("grant store is not valid JSON");
    }
    if (env.v !== 1) throw new GrantStoreError("unsupported grant store version");
    try {
      const decipher = createDecipheriv("aes-256-gcm", this.key(Buffer.from(env.salt, "base64")), Buffer.from(env.iv, "base64"));
      decipher.setAAD(Buffer.from(INFO));
      decipher.setAuthTag(Buffer.from(env.tag, "base64"));
      const plain = Buffer.concat([decipher.update(Buffer.from(env.ct, "base64")), decipher.final()]);
      return JSON.parse(plain.toString("utf8")) as PersistedGrant[];
    } catch {
      throw new GrantStoreError("grant store failed to decrypt: wrong GRANT_STORE_KEY or modified file");
    }
  }

  save(grants: PersistedGrant[]): void {
    const salt = randomBytes(16);
    const iv = randomBytes(12);
    const cipher = createCipheriv("aes-256-gcm", this.key(salt), iv);
    cipher.setAAD(Buffer.from(INFO));
    const ct = Buffer.concat([cipher.update(JSON.stringify(grants), "utf8"), cipher.final()]);
    const env: Envelope = {
      v: 1,
      salt: salt.toString("base64"),
      iv: iv.toString("base64"),
      tag: cipher.getAuthTag().toString("base64"),
      ct: ct.toString("base64"),
    };
    const tmp = `${this.file}.${process.pid}.tmp`;
    writeFileSync(tmp, JSON.stringify(env), { mode: 0o600 });
    chmodSync(tmp, 0o600);
    renameSync(tmp, this.file);
  }

  private key(salt: Buffer): Buffer {
    return Buffer.from(hkdfSync("sha256", this.secret, salt, INFO, 32));
  }
}

export function fromPersisted(p: PersistedGrant): {
  grant: SessionGrant;
  mandateId?: string;
  sessionKey: Hex;
  serialized: string;
  usesRemaining: number;
  consumedAttempts: Set<string>;
} {
  return {
    grant: {
      ...p.grant,
      owner: p.grant.owner as Address,
      maxAmount: BigInt(p.grant.maxAmount),
      maxGas: BigInt(p.grant.maxGas),
      maxGasWei: BigInt(p.grant.maxGasWei),
    },
    ...(p.mandateId ? { mandateId: p.mandateId } : {}),
    sessionKey: p.sessionKey,
    serialized: p.serialized,
    usesRemaining: p.usesRemaining,
    consumedAttempts: new Set(p.consumedAttempts),
  };
}
