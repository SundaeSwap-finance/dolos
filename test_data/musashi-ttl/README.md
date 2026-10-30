# Musashi TTL caller regressions

Two constructed test transactions captured from Musashi, kept as original native
Dijkstra bytes. `confirmed-ttl` is the successful final-pair live transaction
`9b354b0f9b042c677ffdb15665a914499d2bdcef093cd834d5ce0538b212f56f`.
`cross-epoch-ttl` preserves a signed cross-epoch case with its original historical
local parameters; no network success is claimed for that case.

Each case contains only the six inputs needed by the caller test. Shared genesis
initializes the historical domain. Tests exercise admission, evaluation, both
estimation APIs, builtin-call reports, minimum fee and exact upper-slot expiry.
The validator requires the owner's signature and a finite exclusive upper bound
equal to the redeemer's POSIX milliseconds. Current expiry/spendability do not
change offline replay. No current-epoch cutoff is imposed; consensus forecast
enforcement is a separate concern.

The full minimal native proof, reference costs and baseline/fixed results live in
Pallas's `test_data/musashi-ttl` at the dependency revision pinned in `Cargo.toml`.
The successful capture observed 32 subsequent blocks and untouched collateral.
Its V3 model has 350 entries; the cross-epoch case retains its original 251-entry
local model. No CBOR was rewritten or relabelled, and no signing keys are included.
Parameters came from Dolos, not an independent reference RPC. The time mapping is
derived from genesis and observed slots; upstream node build and native LSQ era
history were unavailable. `provenance.json` records source identities/checksums.

```sh
cargo +1.97.0 test --locked -p dolos-cardano --features strict --test dijkstra_ttl
```

`PALLAS_TTL_FIXTURE` optionally selects another collection with the same layout.
The default tests run both cases. Publish the pinned Pallas commit before Dolos;
no path override or path-source lockfile is required. `verification.json` records
compact check results, including known failures. Verbose logs and retry history
are archived outside the committed test inputs.
