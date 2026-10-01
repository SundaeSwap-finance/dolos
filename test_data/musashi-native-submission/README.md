# Native Musashi submission, evaluation and relay

Dolos selects the era from active protocol parameters before decoding submission
or evaluation bytes. Protocol 12.0 selects Dijkstra; unknown later protocols
(including 12.1) reject explicitly. Earlier known eras have bounded mappings;
the earlier-era parameter projection is unchanged. Both native phase one and
phase two use the published Pallas revision
`dc2c81dcced2a316c8aacb8155b8b90fee428e08`. Native evaluation uses its pinned Amaru
`e122ffb2018196ba9c57f42b70b94ada603d3fbc`; earlier eras retain registry
`amaru-uplc` 0.1.0. Rust 1.97 is required. The umbrella Pallas feature does not
forward `unstable` to validation, so Dolos explicitly enables the validator
feature at the same Git revision. This is a dependency, not a Cargo override.

## Captures and historical context

| Case | Transaction ID | Validation slot |
| --- | --- | ---: |
| Control transfer | `f4ed3784097149431498b0df6ceaf13b3a531df88838d1f8f39df1269eb54b20` | 1400007 |
| Settings registration | `da910a9bfbe657724d64b505099010dd47f86fb573aa1d20b4da0367163e94c6` | 1401365 |

Settings is the RealFi application's settings-validator stake credential, not a
Dolos configuration transaction. Its original reference script is resolved from
output 0 of `3171608e4d1b0812131e64e40f670ab7cdeef7f8e0cbad096b0d15dc62b76ba9`;
output 1 supplies spending and collateral. The producer and registration blocks
are retained. The control's original native input is retained separately.

`*.mempool.hex` holds original signed mempool bytes from the retained network capture;
`*.block.hex` holds the transaction extracted from its original ranking block.
Neither body nor witness bytes are changed. Hex follows Pallas's lowercase
convention. `provenance.json` records the exact source paths/revisions and hashes
**decoded bytes** for all CBOR. JSON hashes cover file bytes. `verify.py` checks
the entire retained inventory. The original native bytes and Dijkstra input eras
are used by both validator phases and by mempool/relay. No caller-side Conway
wrapper, adjusted input value, protocol downgrade, or cost-model truncation is
used for native success.

`tests/support/musashi_submission.rs` seeds the relevant original UTxOs, the
recorded epoch-64 active parameters, genesis values, slot and certificate prestate
into a Dolos test domain. The full captured parameter response is retained;
the adapter supplies each field consumed by the supported native rules,
including **all 350 V3 coefficients**, exact rational execution prices and the
Dijkstra genesis's reference-script fee parameters. Transaction redeemers and
declared budgets are untouched. The harness initializes at protocol 12 and
uses the captured system start, one-second slots and 21600-slot epochs. Its
zero-slot time origin gives the same slot/time mapping as the historical eras;
this is not a replay of every historical era transition. Neither capture has a
validity bound needing a forecast. The genesis harness's unrelated accounts,
pots and chain cursor hash are not a reconstruction of the entire historical
ledger. Native validation receives no fabricated treasury account state.

The settings credential's absence before registration is supported by the
retained `registration-certificate-state.json` and history report. These are
reconstructed from the Dolos archive, not an independent node ledger-state dump.
The earlier parameter-history scan and provenance are retained too. Historical
node/evaluator revisions remain unknown; the ledger specification revision
`1587f21a7d1306dc590c2749a5c66232ef66aad0` is a separate identity. The fuller
network capture/commitment and certificate/parameter-history audit remains at
Pallas `dc2c81d`, `test_data/musashi-registration/` and `musashi-phase1/`.
No new network capture or live transaction submission was performed.

## Assertions and proof boundaries

The original transfer and registration pass `SubmitExt::receive_tx`, including
native phase one/signature verification and phase two, at their historical
slots. The registration uses **18485 memory / 4805428 CPU**. The tests also submit a **derived** validity-third/true envelope
accepted by the Dijkstra mempool rule (`pallas-primitives/src/dijkstra/defs.cddl`,
lines 24–27 at the pinned Pallas revision). Captured block-only envelopes are
retained for body/witness identity comparison and explicitly rejected for relay. The validity-third case is a synthetic wrapper test,
not evidence of a separate node capture; false is rejected. Body/witness bytes
and transaction identities remain unchanged.

The production relay worker talks to a loopback recording `PeerServer`, using
both Ephemeral and Redb mempools. Redb is closed and reopened before relay. The
peer checks node-to-node era **7** (stored/traverse era **8**), IDs, sizes and
byte-for-byte transaction bodies for both captures and both accepted mempool wrappers.
This proves the local transport boundary, not acceptance by a live Cardano node.

Synthetic negative tests cover unknown future protocols, malformed/trailing
bytes, tampered signatures, already-registered and pending credentials, native
UnReg, V4 reference scripts and scripted subtransactions. Successful validation
does not commit provisional certificate state. The pending-credential regression
uses two synthetic, signed key registrations with separate funded inputs and the
same credential. Each validates independently; once the first is pending or
inflight, the second must fail specifically as already registered while its input
remains available. Stored account and funding state remain unchanged. This avoids
depending on whether input or certificate checks run first. The generator and
proof limits are recorded under `synthetic_cases` in `provenance.json`; original
capture files remain unchanged. Missing active parameters/genesis
remain errors. The full Pallas native exclusions remain authoritative: protocol
12.0 top-level V3 Reg reference scripts and the audited builtin/input/output
subset only. General V3 deposit/refund context encoding does not establish native
UnReg execution. No V4, scripted-subtransaction, full Dijkstra or consensus claim.

The unchanged `conway-control.hex` comes from Pallas `test_data/conway3.tx`.
Its input is **reconstructed** from Pallas's existing `successful_mainnet_tx`
regression. It verifies earlier-era submission/evaluation behavior; the test
context is not an independently captured historical Conway ledger snapshot.

## Integration baseline

The regression baseline is Dolos `abe2642391006e7e5439a00493d41db5da280a02`.
Native submission uses the published Pallas validator directly; the baseline
contained no vendored validator or Cargo override to remove. The existing
networking implementation is preserved.

The imported `control-provenance.json` and `registration-provenance.json` describe
historical Pallas phase-one evidence. Their relative paths refer to that source
repository, and their scope statements are historical. Editorial labels were
adapted for this repository; `provenance.json` records both original and adapted
report hashes. Captured CBOR, historical parameters and ledger-state evidence
are unchanged.

## Reproduction and checks

```sh
python3 test_data/musashi-native-submission/verify.py
CARGO_TARGET_DIR=target/musashi-native-submission-fixed CARGO_PROFILE_DEV_DEBUG=0 \
  cargo +1.97.0 test --test musashi_submission
CARGO_TARGET_DIR=target/musashi-native-submission-fixed CARGO_PROFILE_DEV_DEBUG=0 \
  cargo +1.97.0 test --lib musashi_recording_peer_receives_original_signed_bytes
python3 test_data/musashi-native-submission/evidence/reproduce-baseline.py
python3 test_data/musashi-native-submission/evidence/run-checks.py
```

The baseline script extracts Dolos `abe26423` without changing the working tree,
copies the same positive assertions and fixture adapter, and uses a separate
Cargo target. The small `baseline.log`, `fixed.log` and `relay.log` files retain
focused runtime evidence; compiler setup errors are not counted as reproduction.
`evidence/verification.json` records commands, results and known failures.
Reproduction scripts write new logs under
`target/musashi-native-submission-evidence/`, keeping baseline and fixed build
outputs separate and the recorded evidence unchanged.
Offline baseline replay requires its dependencies to be cached.

Recorded log checkout/build paths are normalized to placeholders. Full workspace
logs and initial attempts are omitted; commands and results remain in
`evidence/verification.json`. The focused regressions do not establish that all
workspace checks passed.

## Final local outcome (2026-09-30)

- Both captured transactions and accepted mempool wrappers pass: **16 integration
  tests**, plus the recording-peer regression across both storage backends.
- Workspace/all-target/all-feature build passes. The default workspace suite
  passes **1792 tests**, with 33 ignored.
- The required all-features suite reports **1224 passed / 14 failed / 38 ignored**.
  Its two query and twelve Musashi pipeline epoch-coherence failures reproduce
  independently at the original Dolos `abe26423` baseline. No assertion or
  required-suite exclusion was changed to hide them.
- Strict workspace Clippy fails. The full inventory has **19 source warning
  locations, all in unchanged files**; the first strict failure also reproduces
  at the baseline. No new diagnostic from this integration is present. The
  upstream dependency `proc-macro-error2` 2.0.1 emits a separate
  future-compatibility warning.
- Initial concurrent snapshot export/restore processes were killed by SIGKILL.
  All 19 export and 26 restore tests pass when serialized, in both final suites.
  The final gate driver records `RUST_TEST_THREADS=1` and `CARGO_BUILD_JOBS=2`.
- Changed Rust files pass the installed nightly formatter and `git diff --check`.
  Whole-workspace formatting with nightly-2026-09-04 reports differences in 86
  unchanged files. CI's distinct pinned nightly-2026-08-27 was not run. No hosted
  CI, macOS, Windows or live-node acceptance result is claimed.

Full workspace verification remains unsuccessful because of the Clippy and
strict-fixture failures described above. Focused replay and local relay results
do not establish live-node acceptance or full Dijkstra support.
