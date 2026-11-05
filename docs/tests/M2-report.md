# M2 — EVM deposit executor (plain EOA, Alloy)

Date: 2026-11-04. Pair chosen by owner: **Base USDC → Arbitrum USDC**.

## Implemented

* `pcc-evm::AlloyEoaExecutor` implements `SourceExecutor`:
  `simulate` (eth_call + estimateGas + EIP-1559 fees + native/token balances),
  `execute_deposit` (gates → re-simulate → sign → **ledger persist** →
  `eth_sendRawTransaction`), `receipt`.
* Native value transfer vs ERC-20 `transfer(to, amount)`. The approve selector
  `0x095ea7b3` is refused. MEMO deposits are refused (an EOA transfer cannot
  attach a 1Click memo).
* EXACT_OUTPUT: we send `amountIn` (buffer included); 1Click refunds unused
  buffer to `refundTo`.
* `FileLedger`: one signed raw tx per attempt id. Restart rebroadcasts the
  same bytes / hash; a second distinct hash is refused.
* `SOURCE_EXECUTOR_KEY_REF` must be a **0600 file path**. Raw 64-hex in the
  env var is refused. Key bytes are never logged.
* `prepare_deposit` / `execute_approved` / `watch_until` orchestrate quote →
  Rust `validate_quote` → persist attempt → simulate → (stop) → owner-approved
  broadcast → optional `/deposit/submit` → status poll. Broadcast still has to
  pass `ExecutionGates` independently.
* CLI `pcc-deposit preflight` (dry quote via the TS worker). `--allocate-deposit`
  and `execute` are refused in this binary until attempts persist in Postgres
  (M6) **and** the owner approves a specific live tx.

## Commands / tests

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean |
| `cargo test --workspace --all-features` | previous 88 + **19 pcc-evm** (incl. Anvil) |

Anvil tests (local chain id 31337, not a mainnet candidate):

* native transfer mines, receipt success
* restart with the same ledger does **not** double-spend (same hash, same recipient balance)
* `DRY_RUN=true` refuses broadcast
* missing approval ref refuses
* mainnet plan against Anvil RPC refuses (chain mismatch / mainnet gate)
* insufficient balance → `simulation.ok = false`
* fee cap 1 wei → simulation fails, execute refused
* ERC-20: deploy mock token, mint, transfer exact `amountIn` to the deposit address
* full flow: MockOneClick executable quote → validate → simulate → execute →
  `DepositBroadcast` + source tx recorded; second `execute_approved` returns
  the same hash
* `plan_from_quote` refuses dry quotes

## Live calls

**None.** No `dry:false` quote, no mainnet (or testnet) broadcast. Default
gates remain `DRY_RUN=true`, `ENABLE_MAINNET_TRANSFERS=false`. The M1 dry
quote (Base USDC → Arbitrum USDC) still stands as the live evidence for the
route.

## Gate (STOP)

Before any live swap the owner must supply, in chat, **only**:

1. Confirmation of source chain/token and recipient chain/token (proposed:
   Base USDC → Arbitrum USDC, 1.00 USDC out).
2. Public addresses of dedicated test wallets A (payer / `refundTo`) and B
   (merchant recipient). **Not keys.**
3. A maximum **total dollar loss including gas**.
4. Permission to broadcast **one** specific previewed transaction (after we
   print a preflight: deposit address, `amountIn`, estimated fee, destination).

The key for wallet A goes in a 0600 file; `SOURCE_EXECUTOR_KEY_REF` points at
that path. `RPC_URL_BASE` must be a reviewed endpoint. Then, and only then,
`DRY_RUN=false` + `ENABLE_MAINNET_TRANSFERS=true` for that one send.

## Security caveats

* The CLI cannot yet persist attempts across processes (in-memory repo). The
  Anvil tests cover the restart-safe ledger. Postgres-backed execute is M6.
* `dry:false` allocates a real 1Click deposit address with a deadline; we do
  not call it until the gate above is answered.
* Session-key / 7702 path is unchanged and still M3.
