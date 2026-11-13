/**
 * M3 Sepolia preflight. READ-ONLY: getCode + balances. Does not sign a 7702
 * authorization, does not install a delegation, does not send a UserOp.
 *
 * STOP and wait for an explicit owner approval of the printed Kernel
 * implementation, chain, and (later) transfer before any of those happen.
 */
import { existsSync, mkdirSync, writeFileSync, chmodSync } from "node:fs";
import { resolve } from "node:path";
import { createPublicClient, http, isAddress, type Address } from "viem";
import { sepolia } from "viem/chains";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";
import { loadRepoEnv } from "./_env.js";
import {
  KERNEL_7702_DELEGATION_ADDRESS,
  KERNEL_VERSION,
  ENTRY_POINT,
  SUPPORTED_7702_CHAINS,
} from "../src/account-abstraction/pins.js";
import { validateOnboard } from "../src/account-abstraction/validate.js";
import { loadSessionSigner } from "../src/account-abstraction/keys.js";

loadRepoEnv();

const wallet = (process.env["WALLET_A_ADDRESS"] ?? "").trim();
if (!isAddress(wallet)) {
  console.error("WALLET_A_ADDRESS must be set to the dedicated test EOA.");
  process.exit(2);
}

const rpc = process.env["RPC_URL_SEPOLIA"]?.trim() || "https://ethereum-sepolia-rpc.publicnode.com";
const evm = createPublicClient({ chain: sepolia, transport: http(rpc) });

const secretsDir = resolve(process.cwd(), "../../secrets");
const sessionKeyPath = resolve(secretsDir, "session-signer.key");

function ensureSessionKey(): { address: Address; path: string; created: boolean } {
  if (process.env["SESSION_SIGNER_KEY_REF"]?.trim()) {
    const acct = loadSessionSigner(process.env["SESSION_SIGNER_KEY_REF"]);
    return { address: acct.address, path: "(from SESSION_SIGNER_KEY_REF)", created: false };
  }
  if (existsSync(sessionKeyPath)) {
    const acct = loadSessionSigner(sessionKeyPath);
    return { address: acct.address, path: sessionKeyPath, created: false };
  }
  mkdirSync(secretsDir, { recursive: true });
  const pk = generatePrivateKey();
  writeFileSync(sessionKeyPath, pk + "\n", { mode: 0o600 });
  chmodSync(sessionKeyPath, 0o600);
  const acct = privateKeyToAccount(pk);
  return { address: acct.address, path: sessionKeyPath, created: true };
}

const session = ensureSessionKey();
const code = await evm.getCode({ address: wallet as Address });
const { parsed, decision } = validateOnboard(code ?? "0x");
const eth = await evm.getBalance({ address: wallet as Address });

console.log(
  JSON.stringify(
    {
      chain: "sepolia",
      chainId: SUPPORTED_7702_CHAINS.sepolia,
      kernelVersion: KERNEL_VERSION,
      entryPoint: ENTRY_POINT.address,
      entryPointVersion: ENTRY_POINT.version,
      kernelImplementation: KERNEL_7702_DELEGATION_ADDRESS,
      owner: wallet,
      ownerEthWei: eth.toString(),
      codePresent: Boolean(code && code !== "0x"),
      delegation: parsed,
      onboardDecision: decision,
      sessionSigner: session.address,
      sessionKeyCreated: session.created,
      sessionKeyRefHint: session.created
        ? "set SESSION_SIGNER_KEY_REF to secrets/session-signer.key (0600); never put the key in .env"
        : "SESSION_SIGNER_KEY_REF loaded or secrets/session-signer.key exists",
      dryRun: process.env["DRY_RUN"] ?? "(unset → true)",
      enableMainnetTransfers: process.env["ENABLE_MAINNET_TRANSFERS"] ?? "(unset → false)",
    },
    null,
    2,
  ),
);

console.log(
  "\nSTOP. No 7702 authorization was signed and no UserOp was sent. To continue, approve installing Kernel",
  KERNEL_7702_DELEGATION_ADDRESS,
  "on Sepolia for this owner, then a single in-policy test transfer (session key, no second owner signature) plus a negative out-of-policy call.",
);
