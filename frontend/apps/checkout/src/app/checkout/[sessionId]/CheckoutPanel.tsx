"use client";

import {
  CheckoutApiError,
  CheckoutClient,
  merchantContinuation,
  merchantReturnUrl,
  type AllowanceView,
  type ChainDelegationView,
  type CheckoutView,
  type SiweVerifyResponse,
  type SourceAssetView,
} from "@pcc/checkout-sdk";
import { chainIdHex, installKernelDelegation, nativeBalance, tokenBalance, toSignTypedDataV4 } from "@pcc/wallet-onboarding";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { clearDevice, loadDevice, saveDevice } from "../../../device";
import { fundCheckoutWallet, gasReserveWei } from "../../../fund";
import { installKernelViaUserOp } from "../../../kernel";
import { privyConfigured, usePrivyPayer, type PrivyPayer } from "../../../privy";
import { connectWallet, type Eip1193 } from "../../../wallet";
import { BrandMark } from "../../../components/BrandMark";
import { TotpSetup } from "../../../totp-setup";
import { AutopayPanel } from "./AutopayPanel";
import { X402Pay } from "./X402Pay";

const API = "";

const CHAIN_NAMES: Record<number, string> = { 1: "Ethereum", 10: "Optimism", 8453: "Base", 42161: "Arbitrum" };

function formatUnits(base: string, decimals: number): string {
  if (!/^[0-9]+$/.test(base)) return base;
  const padded = base.padStart(decimals + 1, "0");
  const whole = padded.slice(0, padded.length - decimals);
  const frac = padded.slice(padded.length - decimals).replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : whole;
}

/** Decimal string to base units without floating point; null when malformed. */
function parseUnits(value: string, decimals: number): string | null {
  const m = /^([0-9]*)(?:\.([0-9]*))?$/.exec(value.trim());
  if (!m || (!m[1] && !m[2])) return null;
  const frac = m[2] ?? "";
  if (frac.length > decimals) return null;
  const digits = `${m[1] || "0"}${frac.padEnd(decimals, "0")}`.replace(/^0+(?=[0-9])/, "");
  return digits;
}

function multiply(base: string, by: number): string {
  return /^[0-9]+$/.test(base) ? (BigInt(base) * BigInt(by)).toString() : "";
}

const ERRORS: Record<string, string> = {
  quote_failed: "This amount is too small for the bridge. Try at least 0.15 USDC.",
  totp_invalid: "That code is wrong or already used. Wait for the next one.",
  session_grant_required: "Sign an allowance for this payment first.",
  device_unknown: "This device is no longer remembered. Sign in with your wallet.",
  already_enrolled: "This wallet already has an authenticator.",
  preview_required: "No quote for that asset right now. Pick another.",
};

function describe(e: unknown, fallback: string): string {
  if (e instanceof CheckoutApiError) return (e.code && ERRORS[e.code]) || e.message;
  const msg = e instanceof Error ? e.message : "";
  if (/already has an embedded wallet/i.test(msg)) return "That Tachyon wallet is already on this device. Continue again.";
  return msg || fallback;
}

type Step = "load" | "choose" | "x402" | "siwe" | "grant" | "enroll" | "otp" | "done";

export function CheckoutPanel(props: { sessionId: string; embed?: boolean }) {
  if (privyConfigured()) return <PrivyCheckoutPanel {...props} />;
  return <CheckoutPanelView payer={null} {...props} />;
}

function PrivyCheckoutPanel(props: { sessionId: string; embed?: boolean }) {
  const payer = usePrivyPayer();
  return <CheckoutPanelView payer={payer} {...props} />;
}

function CheckoutPanelView({
  sessionId,
  embed = false,
  payer,
}: {
  sessionId: string;
  embed?: boolean;
  payer: PrivyPayer | null;
}) {
  const client = useMemo(() => new CheckoutClient({ baseUrl: API, timeoutMs: 25_000 }), []);
  const provider = useRef<Eip1193 | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [merchant, setMerchant] = useState<string>("");
  const [order, setOrder] = useState<string>("");
  const [amount, setAmount] = useState<string>("");
  const [asset, setAsset] = useState<string>("");
  const [source, setSource] = useState<string>("");
  const [quoted, setQuoted] = useState<string>("");
  const [maxDebit, setMaxDebit] = useState<string>("");
  const [deposit, setDeposit] = useState<string>("");
  const [execution, setExecution] = useState<string>("");
  const [step, setStep] = useState<Step>("load");
  const [sources, setSources] = useState<SourceAssetView[]>([]);
  const [delegation, setDelegation] = useState<ChainDelegationView[]>([]);
  const [pairingUri, setPairingUri] = useState<string>("");
  const [otpauth, setOtpauth] = useState<string>("");
  const [enrolled, setEnrolled] = useState(false);
  const [code, setCode] = useState("");
  const [allowance, setAllowance] = useState<AllowanceView | null>(null);
  const [capInput, setCapInput] = useState("");
  const [uses, setUses] = useState(5);
  const [days, setDays] = useState(7);
  const [busy, setBusy] = useState(false);
  const [signer, setSigner] = useState("");
  const [deviceToken, setDeviceToken] = useState("");
  const [remember, setRemember] = useState(false);
  const [successUrl, setSuccessUrl] = useState<string | null>(null);
  const [intentId, setIntentId] = useState("");
  const [returning, setReturning] = useState(false);
  const [usdcHeld, setUsdcHeld] = useState<bigint | null>(null);
  const [ethHeld, setEthHeld] = useState<bigint | null>(null);
  const [fundsOk, setFundsOk] = useState(false);
  const [fundInput, setFundInput] = useState("");
  const siweLock = useRef(false);

  const refreshAllowance = useCallback(async () => {
    setAllowance(await client.sessionGrantStatus(sessionId).catch(() => null));
  }, [client, sessionId]);

  /** Shared tail of wallet sign-in and remembered-device resume. */
  const afterSignIn = useCallback(
    async (verified: SiweVerifyResponse) => {
      setSigner(verified.address);
      setQuoted(verified.quoted_input);
      setMaxDebit(verified.max_debit);
      setEnrolled(verified.totp_enrolled);
      const listed = await client.sourceAssets(sessionId).catch(() => null);
      if (listed) {
        setSources(listed.assets);
        if (listed.selected) setSource(listed.selected);
      }
      const status = await client.delegation(sessionId).catch(() => null);
      setDelegation(status?.chains ?? []);
      await refreshAllowance();
      setStep("grant");
    },
    [client, refreshAllowance, sessionId],
  );

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const view = await client.checkout(sessionId);
        if (cancelled) return;
        setMerchant(view.merchant_name);
        setOrder(view.merchant_order_id);
        setAmount(view.output_amount);
        setAsset(view.destination_asset);
        setSource(view.source_asset);
        setQuoted(view.quoted_input ?? "");
        setMaxDebit(view.max_debit ?? "");
        setDeposit(view.deposit_address ?? "");
        setSuccessUrl(view.success_url);
        setIntentId(view.intent_id);
        const device = loadDevice();
        const resumed = device ? await client.resumeDevice(sessionId, device.token).catch(() => null) : null;
        if (cancelled) return;
        if (device && resumed?.address.toLowerCase() === device.address.toLowerCase()) {
          setDeviceToken(device.token);
          await afterSignIn(resumed);
          return;
        }
        if (device) clearDevice();
        setStep("choose");
      } catch (e) {
        if (!cancelled) setError(e instanceof CheckoutApiError ? "Checkout session not found or expired." : "API unreachable.");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [afterSignIn, client, sessionId]);

  const run = useCallback(async (fallback: string, body: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await body();
    } catch (e) {
      setError(describe(e, fallback));
    } finally {
      setBusy(false);
    }
  }, []);

  const toCode = useCallback(async () => {
    if (enrolled) {
      setStep("otp");
      return;
    }
    const enrollment = await client.enrollTotp(sessionId);
    setOtpauth(enrollment.otpauth_uri);
    setStep("enroll");
  }, [client, enrolled, sessionId]);

  const signIn = useCallback(
    (mode: "privy" | "browser" = payer ? "privy" : "browser") =>
      run("SIWE failed", async () => {
        if (mode === "privy") {
          if (!payer) throw new Error("Privy is not configured.");
          if (!payer.authenticated) {
            payer.login();
            return;
          }
          const wallet = await payer.provider();
          provider.current = wallet;
          const accounts = (await wallet.request({ method: "eth_requestAccounts" })) as string[];
          const address = accounts[0];
          if (!address) throw new Error("Wallet returned no address.");
          const challenge = await client.startSiwe(sessionId, address);
          const signature = (await wallet.request({ method: "personal_sign", params: [challenge.message, address] })) as string;
          await afterSignIn(await client.verifySiwe(sessionId, challenge.message, signature));
          return;
        }
        const wallet = await connectWallet(setPairingUri);
        provider.current = wallet;
        setPairingUri("");
        const accounts = (await wallet.request({ method: "eth_requestAccounts" })) as string[];
        const address = accounts[0];
        if (!address) throw new Error("Wallet returned no address.");
        const challenge = await client.startSiwe(sessionId, address);
        const signature = (await wallet.request({ method: "personal_sign", params: [challenge.message, address] })) as string;
        await afterSignIn(await client.verifySiwe(sessionId, challenge.message, signature));
      }),
    [afterSignIn, client, payer, run, sessionId],
  );

  useEffect(() => {
    if (!payer || !payer.ready || !payer.authenticated || step !== "siwe" || siweLock.current) return;
    siweLock.current = true;
    void signIn("privy");
  }, [payer, signIn, step]);

  /** The signed-in wallet, connecting it first when the session came from a remembered device. */
  const walletFor = useCallback(async (): Promise<{ wallet: Eip1193; address: string }> => {
    if (payer) {
      if (!payer.authenticated) {
        payer.login();
        throw new Error("Sign in to your Tachyon wallet to continue.");
      }
      const wallet = await payer.provider();
      const accounts = (await wallet.request({ method: "eth_accounts" })) as string[];
      const address = accounts[0];
      if (!address) throw new Error("Wallet returned no address.");
      if (signer && address.toLowerCase() !== signer.toLowerCase()) {
        throw new Error(`Sign in with the Tachyon wallet ${signer}.`);
      }
      provider.current = wallet;
      return { wallet, address };
    }
    let wallet = provider.current;
    if (!wallet) {
      wallet = await connectWallet(setPairingUri);
      setPairingUri("");
      await wallet.request({ method: "eth_requestAccounts" });
    }
    const accounts = (await wallet.request({ method: "eth_accounts" })) as string[];
    const address = accounts[0];
    if (!address) throw new Error("Wallet returned no address.");
    if (address.toLowerCase() !== signer.toLowerCase()) {
      throw new Error(`Switch your wallet to ${signer}, the account signed in to this checkout.`);
    }
    provider.current = wallet;
    return { wallet, address };
  }, [payer, signer]);

  const switchWallet = useCallback(() => {
    if (deviceToken) void client.forgetDevice(deviceToken).catch(() => undefined);
    clearDevice();
    setDeviceToken("");
    provider.current = null;
    siweLock.current = false;
    setSigner("");
    setAllowance(null);
    setEnrolled(false);
    setCode("");
    setUsdcHeld(null);
    setEthHeld(null);
    setFundsOk(false);
    setError(null);
    setStep("siwe");
    if (payer?.authenticated) void payer.logout();
  }, [client, deviceToken, payer]);

  const returnToMerchant = useCallback(
    (view: Pick<CheckoutView, "status" | "attempt_status">, executionNow: string) => {
      if (!successUrl) return;
      const result = merchantContinuation({
        intentStatus: view.status,
        attemptStatus: view.attempt_status,
        execution: executionNow,
      });
      const target = merchantReturnUrl(successUrl, window.location.origin, {
        intentId,
        sessionId,
        result,
        intentStatus: view.status,
        attemptStatus: view.attempt_status,
        execution: executionNow,
      });
      setReturning(true);
      if (window.parent !== window) {
        window.parent.postMessage(
          { source: "pcc-checkout", kind: "done", result, intentId, sessionId, status: view.status },
          "*",
        );
        return;
      }
      window.location.assign(target);
    },
    [intentId, sessionId, successUrl],
  );

  useEffect(() => {
    if (step !== "done" || !successUrl) return;
    let cancelled = false;
    const tick = async () => {
      const view = await client.checkout(sessionId).catch(() => null);
      if (!view || cancelled) return;
      const result = merchantContinuation({
        intentStatus: view.status,
        attemptStatus: view.attempt_status,
        execution,
      });
      if (result === "pending") return;
      returnToMerchant(view, execution);
    };
    const id = setInterval(() => void tick(), 3_000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [client, execution, returnToMerchant, sessionId, step, successUrl]);

  const installDelegation = useCallback(
    () =>
      run("Could not install the delegation", async () => {
        const selectedAsset = sources.find((s) => s.asset_id === source);
        const status = selectedAsset ? delegation.find((d) => d.chain_id === selectedAsset.chain_id) : undefined;
        if (!status || status.state !== "none") return;
        const { wallet, address } = await walletFor();
        if (payer) {
          await installKernelViaUserOp({
            provider: wallet,
            address: address as `0x${string}`,
            chainId: status.chain_id,
            signAuth: () => payer.signKernelAuth(status.chain_id),
          });
        } else {
          await installKernelDelegation(wallet, address as `0x${string}`, status.chain_id);
        }
        setDelegation((await client.delegation(sessionId)).chains);
      }),
    [client, delegation, payer, run, sessionId, source, sources, walletFor],
  );

  const refreshFunds = useCallback(async () => {
    const selectedAsset = sources.find((s) => s.asset_id === source);
    if (!selectedAsset || !signer) return;
    const [usdc, eth] = await Promise.all([
      tokenBalance(selectedAsset.chain_id, selectedAsset.token_address, signer),
      nativeBalance(selectedAsset.chain_id, signer),
    ]);
    setUsdcHeld(usdc);
    setEthHeld(eth);
    if (!quoted || !/^[0-9]+$/.test(quoted)) {
      setFundsOk(false);
      return;
    }
    const needEth = (delegation.find((d) => d.chain_id === selectedAsset.chain_id)?.state ?? "none") === "none";
    setFundsOk(usdc >= BigInt(quoted) && (!needEth || eth >= gasReserveWei(selectedAsset.chain_id)));
  }, [delegation, quoted, signer, source, sources]);

  useEffect(() => {
    setUsdcHeld(null);
    setEthHeld(null);
    setFundsOk(false);
    setFundInput("");
  }, [source]);

  useEffect(() => {
    if (fundInput !== "") return;
    if (!quoted || !/^[0-9]+$/.test(quoted) || usdcHeld === null) return;
    const tokenDecimals = sources.find((s) => s.asset_id === source)?.decimals ?? 6;
    const remain = BigInt(quoted) > usdcHeld ? BigInt(quoted) - usdcHeld : BigInt(quoted);
    setFundInput(formatUnits(remain.toString(), tokenDecimals));
  }, [quoted, usdcHeld, fundInput, source, sources]);

  useEffect(() => {
    if (step !== "grant" || !signer || !payer) return;
    void refreshFunds().catch(() => undefined);
  }, [payer, refreshFunds, signer, step]);

  const fundFromMetaMask = useCallback(
    () =>
      run("Could not fund the checkout wallet", async () => {
        const selectedAsset = sources.find((s) => s.asset_id === source);
        if (!selectedAsset) throw new Error("Pick a source asset first.");
        if (!quoted || !/^[0-9]+$/.test(quoted)) throw new Error("No quote for that asset right now.");
        const needEth =
          (delegation.find((d) => d.chain_id === selectedAsset.chain_id)?.state ?? "none") === "none";
        const haveEth = ethHeld ?? 0n;
        const entered = parseUnits(fundInput.trim() || "0", selectedAsset.decimals);
        if (entered === null) throw new Error("Enter how much USDC MetaMask should send.");
        const usdcSend = BigInt(entered);
        const ethNeed = needEth && haveEth < gasReserveWei(selectedAsset.chain_id) ? gasReserveWei(selectedAsset.chain_id) - haveEth : 0n;
        if (usdcSend <= 0n && ethNeed <= 0n) {
          setFundsOk(true);
          return;
        }
        await fundCheckoutWallet({
          onPairingUri: setPairingUri,
          chainId: selectedAsset.chain_id,
          token: selectedAsset.token_address,
          to: signer as `0x${string}`,
          usdc: usdcSend,
          eth: ethNeed,
        });
        setPairingUri("");
        await refreshFunds();
      }),
    [delegation, ethHeld, fundInput, quoted, refreshFunds, run, signer, source, sources],
  );

  const chooseSource = useCallback(
    (assetId: string) =>
      run("Asset change failed", async () => {
        const picked = await client.selectSourceAsset(sessionId, assetId);
        setSource(picked.source_asset);
        setQuoted(picked.quoted_input);
        setMaxDebit(picked.max_debit);
        setCapInput("");
        await refreshAllowance();
        setStep("grant");
        if (!picked.max_debit) setError("No quote for that asset right now. Pick another or retry.");
      }),
    [client, refreshAllowance, run, sessionId],
  );

  const selected = sources.find((s) => s.asset_id === source);
  const decimals = selected?.decimals ?? 6;
  const capBase = capInput ? parseUnits(capInput, decimals) : maxDebit;
  const capTooLow = !!capBase && !!maxDebit && BigInt(capBase) < BigInt(maxDebit);

  const grantSpend = useCallback(
    () =>
      run("Allowance signature failed", async () => {
        if (!capBase) throw new Error("Enter a per-payment cap like 2.50.");
        const { wallet, address } = await walletFor();
        const grant = await client.prepareSessionGrant(sessionId, { max_amount: capBase, uses, valid_days: days });
        await wallet.request({ method: "wallet_switchEthereumChain", params: [{ chainId: chainIdHex(grant.chain_id) }] });
        const signature = (await wallet.request({
          method: "eth_signTypedData_v4",
          params: [address, toSignTypedDataV4(grant.typed_data)],
        })) as string;
        setAllowance(await client.enableSessionGrant(sessionId, grant.grant_id, signature));
        await toCode();
      }),
    [capBase, client, days, run, sessionId, toCode, uses, walletFor],
  );

  const payWithAllowance = useCallback(() => run("Could not continue", toCode), [run, toCode]);

  const revoke = useCallback(
    () =>
      run("Revoke failed", async () => {
        await client.revokeSessionGrant(sessionId);
        setAllowance(null);
      }),
    [client, run, sessionId],
  );

  const confirmAndPay = useCallback(
    () =>
      run("TOTP failed", async () => {
        if (step === "enroll") {
          await client.confirmTotp(sessionId, code);
          setEnrolled(true);
          setOtpauth("");
          setStep("otp");
          setCode("");
          return;
        }
        try {
          const auth = await client.authorize(sessionId, code, undefined, { rememberDevice: remember && !deviceToken });
          if (auth.device_token) {
            saveDevice({ token: auth.device_token, address: signer });
            setDeviceToken(auth.device_token);
          }
          setQuoted(auth.quoted_input);
          setDeposit(auth.deposit_address ?? "");
          setExecution(auth.execution);
          setStep("done");
        } catch (e) {
          if (e instanceof CheckoutApiError && e.code === "session_grant_required") {
            setCode("");
            await refreshAllowance();
            setStep("grant");
          }
          throw e;
        }
      }),
    [client, code, deviceToken, refreshAllowance, remember, run, sessionId, signer, step],
  );

  const sourceLabel = selected ? `${CHAIN_NAMES[selected.chain_id] ?? selected.chain_id} ${selected.symbol}` : "Base USDC";
  const show = (v: string) => (v && selected ? `${formatUnits(v, selected.decimals)} ${selected.symbol}` : v || "…");
  const chainStatus = selected ? delegation.find((d) => d.chain_id === selected.chain_id) : undefined;
  const blocked = chainStatus !== undefined && !chainStatus.ready && chainStatus.state !== "unknown";

  const stepIndex = ["siwe", "grant", "enroll", "otp", "done"].indexOf(step);
  const payout = amount ? formatUnits(amount, 6) : "…";
  const failed = execution === "execute_rejected" || execution === "simulation_failed";
  const doneTone =
    execution === "broadcast" ? "positive" : execution === "awaiting_approval" || execution === "gated" ? "notice" : failed ? "negative" : "information";
  const doneLabel =
    execution === "broadcast"
      ? "Payment sent"
      : execution === "gated"
        ? "Authorized · live send off"
        : execution === "awaiting_approval"
          ? "Waiting for operator"
          : execution || "…";

  return (
    <main className={embed ? "pay-shell pay-shell-embed" : "pay-shell"}>
      <div className="pay-sheet">
        <header className="pay-header">
          <BrandMark />
          <h1>Pay {merchant || "merchant"}</h1>
          {step === "load" && !error ? (
            <div className="pay-skeleton" aria-busy="true" aria-label="Loading checkout">
              <span />
              <span />
              <span />
            </div>
          ) : (
            <>
              <p className="pay-amount">
                {payout}
                <span className="pay-amount-currency">USDC</span>
              </p>
              <dl className="pay-info">
                <div>
                  <dt>Order</dt>
                  <dd>{order || "…"}</dd>
                </div>
                <div>
                  <dt>You send</dt>
                  <dd>
                    {show(quoted)} · {sourceLabel}
                  </dd>
                </div>
              </dl>
            </>
          )}
          {step !== "load" && step !== "choose" && step !== "x402" ? (
            <div className="pay-steps" aria-hidden="true">
              {["siwe", "grant", "otp", "done"].map((id, i) => (
                <span key={id} data-on={String(stepIndex >= (id === "otp" && step === "enroll" ? 2 : i))} />
              ))}
            </div>
          ) : null}
        </header>
        <div className="pay-body">
          {error ? (
            <p role="alert" className="pay-alert pay-alert-negative">
              {error}
            </p>
          ) : null}
          {pairingUri ? (
            <p className="pay-muted">
              If the wallet QR did not open, paste this into your wallet: <span className="pay-wallet">{pairingUri}</span>
            </p>
          ) : null}
          {signer && step !== "siwe" ? (
            <p className="pay-muted">
              <span className="pay-wallet">{signer}</span>
              {deviceToken ? " · remembered" : ""}{" "}
              <button type="button" className="pay-btn pay-btn-tertiary" onClick={switchWallet} disabled={busy}>
                Different wallet
              </button>
            </p>
          ) : null}
          {step === "choose" ? (
            <div className="pay-actions">
              <button type="button" className="pay-btn" disabled={busy} onClick={() => setStep("x402")}>
                Private x402
              </button>
              <p className="pay-muted">One wallet transfer. No authenticator and no delegation.</p>
              <button type="button" className="pay-btn pay-btn-secondary" disabled={busy} onClick={() => setStep("siwe")}>
                Private checkout
              </button>
              <p className="pay-muted">Standing allowance plus authenticator, so later payments skip the wallet popup.</p>
            </div>
          ) : null}
          {step === "x402" ? (
            <X402Pay
              sessionId={sessionId}
              busy={busy}
              onBusy={setBusy}
              onError={setError}
              onBack={() => {
                setError("");
                setStep("choose");
              }}
              onPaid={(exec, intentStatus) => {
                setExecution(exec);
                setError("");
                setStep("done");
                void client
                  .checkout(sessionId)
                  .then((view) => returnToMerchant({ ...view, status: intentStatus || view.status }, exec))
                  .catch(() => undefined);
              }}
            />
          ) : null}
          {step === "siwe" ? (
            <div className="pay-actions">
              {payer ? (
                <>
                  <button
                    type="button"
                    className="pay-btn"
                    onClick={() => void signIn("privy")}
                    disabled={busy || !payer.ready}
                  >
                    {busy ? "Waiting on your wallet…" : payer.authenticated ? "Continue with Tachyon wallet" : "Create Tachyon wallet"}
                  </button>
                  <p className="pay-muted">
                    Email or Google. Kernel and the allowance stay on this wallet. MetaMask only sends USDC to it.
                  </p>
                </>
              ) : (
                <button type="button" className="pay-btn" onClick={() => void signIn("browser")} disabled={busy}>
                  {busy ? "Waiting on your wallet…" : "Continue with wallet"}
                </button>
              )}
            </div>
          ) : null}
          {step === "grant" ? (
            <section className="pay-stack">
              {sources.length > 0 ? (
                <label className="pay-field">
                  Pay with
                  <select value={source} disabled={busy} onChange={(e) => void chooseSource(e.target.value)}>
                    {sources.map((s) => (
                      <option key={s.asset_id} value={s.asset_id}>
                        {CHAIN_NAMES[s.chain_id] ?? s.chain_id} {s.symbol}
                      </option>
                    ))}
                  </select>
                </label>
              ) : null}
              {chainStatus ? (
                <p className={blocked && chainStatus.state !== "none" ? "pay-alert pay-alert-negative" : "pay-muted"}>
                  {CHAIN_NAMES[chainStatus.chain_id] ?? chainStatus.chain_id}: {chainStatus.detail}
                  {blocked && chainStatus.state !== "none"
                    ? ` Pick another chain in Pay with, or Different wallet. Tachyon will not overwrite the existing ${CHAIN_NAMES[chainStatus.chain_id] ?? "chain"} delegation.`
                    : ""}
                </p>
              ) : null}
              {payer && signer && selected ? (
                <div className="pay-stack">
                  <p className="pay-muted">
                    Checkout wallet {signer.slice(0, 6)}…{signer.slice(-4)} holds{" "}
                    {usdcHeld === null ? "…" : `${formatUnits(usdcHeld.toString(), decimals)} ${selected.symbol}`}
                    {ethHeld === null ? "" : ` and ${formatUnits(ethHeld.toString(), 18)} ETH`} on{" "}
                    {CHAIN_NAMES[selected.chain_id] ?? selected.chain_id}.
                  </p>
                  {usdcHeld === null ? (
                    <p className="pay-muted">Checking checkout wallet balances…</p>
                  ) : (
                    <div className="pay-stack">
                      {fundsOk ? <p className="pay-alert pay-alert-positive">Funds received on the checkout wallet.</p> : null}
                      <label className="pay-field">
                        Top up amount ({selected.symbol})
                        <input
                          value={fundInput}
                          onChange={(e) => setFundInput(e.target.value)}
                          inputMode="decimal"
                          autoComplete="off"
                          aria-describedby="pay-topup-hint"
                        />
                      </label>
                      <p id="pay-topup-hint" className="pay-muted">
                        This payment needs {quoted ? show(quoted) : "a quote"}. Send more if you want later charges to use
                        this wallet.
                      </p>
                      <div className="pay-actions">
                        <button
                          type="button"
                          className="pay-btn"
                          onClick={() => void fundFromMetaMask()}
                          disabled={busy || !quoted || parseUnits(fundInput.trim() || "0", decimals) === null}
                        >
                          {busy ? "Waiting on MetaMask…" : "Send USDC from MetaMask"}
                        </button>
                        <p className="pay-muted">
                          MetaMask sends that USDC
                          {chainStatus?.state === "none" ? " and a little ETH for Kernel gas" : ""} to this address. It
                          does not install Kernel.
                        </p>
                      </div>
                    </div>
                  )}
                </div>
              ) : null}
              {chainStatus?.state === "none" ? (
                <div className="pay-actions">
                  <button
                    type="button"
                    className="pay-btn"
                    onClick={() => void installDelegation()}
                    disabled={busy || Boolean(payer && !fundsOk)}
                  >
                    Install delegation on {CHAIN_NAMES[chainStatus.chain_id] ?? chainStatus.chain_id}
                  </button>
                  <p className="pay-muted">
                    {payer
                      ? "Your Tachyon wallet signs Kernel. The bundler submits it. Gas is paid from the ETH MetaMask sent here."
                      : "One type-4 transaction in your wallet. You pay the gas. MetaMask cannot do this step from a website; connect a wallet that can sign EIP-7702, then Sign allowance here."}
                  </p>
                </div>
              ) : null}
              {allowance ? (
                <div className="pay-stack">
                  <p className="pay-muted">
                    Allowance up to {show(allowance.max_amount)} · {allowance.uses_remaining} left · until{" "}
                    {new Date(allowance.valid_until * 1000).toLocaleDateString()}
                  </p>
                  {allowance.covers_this_payment ? (
                    <div className="pay-actions">
                      <button type="button" className="pay-btn" onClick={() => void payWithAllowance()} disabled={busy}>
                        Pay with code
                      </button>
                      <button type="button" className="pay-btn pay-btn-secondary" onClick={() => void revoke()} disabled={busy}>
                        Revoke allowance
                      </button>
                    </div>
                  ) : (
                    <p role="alert" className="pay-alert pay-alert-notice">
                      This payment is above the allowance cap. Sign a new one below.
                    </p>
                  )}
                  {allowance.covers_this_payment ? null : (
                    <button type="button" className="pay-btn pay-btn-secondary" onClick={() => void revoke()} disabled={busy}>
                      Revoke allowance
                    </button>
                  )}
                </div>
              ) : null}
              {!blocked && !allowance?.covers_this_payment ? (
                <div className="pay-stack">
                  <p className="pay-muted">Sign once. Later payments from this wallet need only your authenticator code.</p>
                  <label className="pay-field">
                    Per-payment cap ({selected?.symbol ?? "token"})
                    <input
                      value={capInput || (maxDebit ? formatUnits(maxDebit, decimals) : "")}
                      onChange={(e) => setCapInput(e.target.value)}
                      inputMode="decimal"
                    />
                  </label>
                  <div className="pay-row">
                    <label className="pay-field">
                      Payments
                      <input type="number" min={1} max={20} value={uses} onChange={(e) => setUses(Number(e.target.value))} />
                    </label>
                    <label className="pay-field">
                      Days
                      <input type="number" min={1} max={30} value={days} onChange={(e) => setDays(Number(e.target.value))} />
                    </label>
                  </div>
                  <p className={capTooLow || !capBase ? "pay-alert pay-alert-negative" : "pay-muted"}>
                    {!capBase
                      ? "Cap must be a number."
                      : capTooLow
                        ? `Cap must be at least ${show(maxDebit)}.`
                        : `Most this can move: ${show(multiply(capBase, uses))} over ${days} day${days === 1 ? "" : "s"}.`}
                  </p>
                  <div className="pay-actions">
                    <button
                      type="button"
                      className="pay-btn"
                      onClick={() => void grantSpend()}
                      disabled={
                        busy ||
                        blocked ||
                        !maxDebit ||
                        !capBase ||
                        capTooLow ||
                        uses < 1 ||
                        uses > 20 ||
                        days < 1 ||
                        days > 30 ||
                        Boolean(payer && !fundsOk)
                      }
                    >
                      {busy ? "Waiting on your wallet…" : "Sign allowance"}
                    </button>
                  </div>
                </div>
              ) : null}
            </section>
          ) : null}
          {step === "enroll" ? (
            <section className="pay-stack">
              <p className="pay-muted">Add Tachyon to your authenticator. Shown once.</p>
              {otpauth ? <TotpSetup otpauth={otpauth} /> : null}
              <label className="pay-field">
                Enrollment code
                <input
                  className="pay-otp"
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  maxLength={6}
                  value={code}
                  onChange={(e) => setCode(e.target.value.replace(/\D/g, "").slice(0, 6))}
                />
              </label>
              <div className="pay-actions">
                <button type="button" className="pay-btn" onClick={() => void confirmAndPay()} disabled={busy || code.length !== 6}>
                  Confirm
                </button>
              </div>
            </section>
          ) : null}
          {step === "otp" ? (
            <section className="pay-stack">
              <label className="pay-field">
                Payment code
                <input
                  className="pay-otp"
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  maxLength={6}
                  value={code}
                  onChange={(e) => setCode(e.target.value.replace(/\D/g, "").slice(0, 6))}
                />
              </label>
              <div className="pay-actions">
                <button type="button" className="pay-btn" onClick={() => void confirmAndPay()} disabled={busy || code.length !== 6}>
                  {busy ? "Authorizing…" : "Pay"}
                </button>
                {!deviceToken ? (
                  <label className="pay-muted">
                    <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} /> Remember this device
                    for 30 days
                  </label>
                ) : null}
                <button
                  type="button"
                  className="pay-btn pay-btn-secondary"
                  disabled={busy}
                  onClick={() =>
                    void run("Could not replace the authenticator", async () => {
                      await client.resetTotp(sessionId);
                      setEnrolled(false);
                      setCode("");
                      const enrollment = await client.enrollTotp(sessionId);
                      setOtpauth(enrollment.otpauth_uri);
                      setStep("enroll");
                    })
                  }
                >
                  I lost my authenticator
                </button>
              </div>
            </section>
          ) : null}
          {step === "done" ? (
            <section className="pay-stack">
              <p>
                <span className={`pay-badge pay-badge-${doneTone}`}>{doneLabel}</span>
              </p>
              {successUrl ? (
                <div className="pay-actions">
                  <button
                    type="button"
                    className="pay-btn"
                    disabled={returning}
                    onClick={() =>
                      void client
                        .checkout(sessionId)
                        .then((view) => returnToMerchant(view, execution))
                        .catch((e) => setError(describe(e, "Could not return to the merchant")))
                    }
                  >
                    {returning ? "Returning…" : "Return to merchant"}
                  </button>
                  <p className="pay-muted">The merchant confirms the payment on its server before completing the order.</p>
                </div>
              ) : null}
            </section>
          ) : null}
          {enrolled && (step === "grant" || step === "otp" || step === "done") ? (
            <details className="pay-details">
              <summary>Set up autopay</summary>
              <AutopayPanel
                client={client}
                sessionId={sessionId}
                walletFor={walletFor}
                merchant={merchant}
                show={show}
                parse={(v) => parseUnits(v, decimals)}
                toDecimal={(v) => formatUnits(v, decimals)}
                defaultCap={maxDebit}
              />
            </details>
          ) : null}
        </div>
      </div>
    </main>
  );
}
