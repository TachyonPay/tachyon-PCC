# Example: confidential swap

NEAR Intents 1Click `confidentiality: basic|advanced` EXACT_OUTPUT quotes are
already exercised by M1 (dry) and M4 (session-key live, owner-approved). This
folder is a recipe, not a second executor.

```sh
cd services/integrations-ts
pnpm live:dry-quote   # dry:true, confidentiality:basic, signature verified
```

Override `ORIGIN_ASSET`, `DESTINATION_ASSET`, `AMOUNT_OUT`. The Rust API still
re-validates `pcc_payments::validate_quote` before anything is persisted.
Executable (non-dry) quotes stay behind authorization + execution gates; there
is no unattended mainnet send from this example.
