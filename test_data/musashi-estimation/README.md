# Builder evaluation through Dolos gRPC

Both UTxO RPC versions now estimate execution costs through `SubmitExt::estimate_tx`
and `ChainLogic::estimate_tx`, returning an `EvalReport` rather than a validated
mempool transaction. Native Dijkstra calls Pallas `phase2::estimate_tx`; earlier
eras keep the existing phase-two evaluator. Pallas is pinned to published commit
`3b85c27dd665474e49adcbd328364e39c26bf27f` with `phase2,unstable` enabled. The lock
update changes only the 13 Pallas source revisions, not dependency versions.

Estimation does not require final key witnesses or sufficient declared budgets.
Each native script gets an independent finite protocol execution limit. Returned
estimates may sum above the transaction maximum, so successful execution is not
admission approval. Submission still performs phase one, withdrawal reservations,
and budget-enforcing phase two before accepting the original bytes into the mempool.
The existing Cardano `evaluate_tx` and CLI validation behavior are retained.

RPC responses explicitly map the six redeemer purposes, preserve indices and
consumed units (including failures), expose failure messages and logs, and sum
units with overflow detection. A failed report entry populates RPC `errors` even
when Pallas returned `Ok(report)`; missing diagnostics get a fallback message.
Request-shape errors remain tonic `InvalidArgument`; evaluation errors remain in
the existing `TxEval.errors` response. No protobuf schema changes are required.

## Fixture and proof

No new CBOR files or captures were created. Tests reuse
[`../musashi-native-submission`](../musashi-native-submission/README.md), with
its original provenance, network magic 164, protocol 12.0, epoch-64 parameters,
all 350 Plutus V3 coefficients and original native output eras. The settings
registration `da910a9bfbe657724d64b505099010dd47f86fb573aa1d20b4da0367163e94c6`
uses exactly **18,485 memory / 4,805,428 steps**. “Settings” refers to RealFi's
application settings validator, not a Dolos or ledger setting.

The captured transaction, producer and input/parameter files remain byte-identical.
The existing fixture helper reconstructs the documented historical context; it
is not a current ledger snapshot. Historical node/evaluator builds remain unknown.

`src/serve/grpc/submit_tests.rs` is included by both versioned service modules.
It calls the actual tonic service methods in process. Identical test source on
baseline Dolos `b26c8a7cc547e2242b6698b634300acd40fb7a04`: **6 pass / 14 runtime
failures**. Fixed: **20 pass**. The baseline fails at the wrong purpose for the
unchanged capture, missing witnesses/integrity for builder derivatives, missing
failure entries after exhaustion, and budget-enforcing phase two for a correctly
signed synthetic transaction. The negative/control cases pass on both.

Synthetic derivatives are created in memory and explicitly separated from capture:
unsigned witnesses, zero/insufficient/excessive budgets, exhausted protocol limits,
missing inputs, malformed requests, and pending/inflight input lookup. A synthetic
signed case changes the funding/collateral address to a deterministic test key,
updates explicit signer guards if present, and recomputes integrity/signatures.
It first passes both validation phases at the measured budget, then fails exactly
with `Phase2ValidationRejected` at zero budget while RPC estimation still succeeds.
Neither these transactions nor the historical captures were submitted to a network.

An additional **6 mapping tests** (three per RPC version) cover all purpose enums,
partial failure, error fallback, traces, empty reports and arithmetic overflow.
These use synthetic reports: the native evaluator's Trace builtin remains outside
its supported subset. Conway coverage reuses the original control transaction
with the existing reconstructed input, and proves legacy routing and submission.
Pending/inflight tests use the production ephemeral mempool because ToyDomain's
mempool lifecycle methods are no-ops. Their direct producer insertion tests lookup,
not producer admission. No full Dijkstra or live-node compatibility claim is made.

## Reproduction

From the Dolos root, with Rust 1.97.0 and dependencies cached:

```sh
python3 test_data/musashi-estimation/verify.py
CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$PWD/target/musashi-s07-fixed" cargo test --offline --locked -p dolos --lib estimation_ -- --nocapture
```

Prepare the baseline in a fresh directory (the local session retained it at
`target/dolos-estimation-baseline-source`):

```sh
mkdir -p target/estimation-baseline-replay
git archive b26c8a7cc547e2242b6698b634300acd40fb7a04 | tar -x -C target/estimation-baseline-replay
cp src/serve/grpc/submit_tests.rs target/estimation-baseline-replay/src/serve/grpc/
(cd target/estimation-baseline-replay && patch -p1 < ../../test_data/musashi-estimation/baseline-wiring.patch)
CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$PWD/target/musashi-s07-baseline" cargo test --manifest-path target/estimation-baseline-replay/Cargo.toml --offline --locked -p dolos --lib estimation_rpc_ -- --nocapture
```

The patch only includes the new tests in the two service modules; baseline
production code and dependency pin are unchanged. Baseline and fixed builds use
separate targets. `baseline.log` and `fixed.log` contain the runtime output with
build chatter removed and whitespace normalized. Initial test-development issues
(wrong binary target, error-string assertion, synthetic fee setup) are not counted
as reproductions. Final source hashes match between the two checkouts.

`provenance.json` records original fixture dependencies, decoded-CBOR hashes,
synthetic derivations, test/log hashes and limitations. `verification.json` records
the final repository checks and known failures separately. Full local logs and
runner are in `target/dolos-estimation-checks`; essential regression evidence is
tracked here and does not depend on `/tmp`.
