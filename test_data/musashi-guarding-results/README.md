# Guarding result conversion coverage

This is synthetic caller-boundary coverage for the Guarding-capable Pallas
`TxEvalResult` API. No new captured CBOR is introduced or modified.

The v1 tests in `src/serve/grpc/v1/submit.rs` construct evaluator results and
run the production `tx_eval_to_u5c` conversion. They cover successful Guarding
results, failure messages and their fallback, original nonzero indices, ordered
logs with purpose/index labels, individual and total execution units, and a
failed Guarding result followed by a successful certificate result. The shared
mapping tests retain the six earlier purposes, empty reports and overflow cases.

The v1alpha/v1beta tests reject Guarding in each position of a mixed report,
for both success and failure. They require a clear unsupported-purpose error
and no partial redeemers, traces or total budget. The ledger-oracle helper test
checks ledger tag 6 and the original index; RPC v1 uses purpose value 7.

The budgets and messages in these tests are deliberately synthetic. The shared
helper uses 18,485 memory and 4,805,428 steps; the failure case uses 11 and 23.
These numbers are not measurements of Guarding execution. Logs are constructed
explicitly, including cases unavailable from the supported native Trace subset.

This increment enables representation and conversion of results. It does not
enable Guarding dispatch, V4 transaction execution, datum/context rules, batches,
or submission. There is no native Guarding network capture or historical runtime
red/green reproduction for this increment. The original Conway-tag API cannot
construct a Guarding result; a compilation failure is not a bug reproduction.
Existing captured registration/TTL/native-reference tests remain regression
coverage for previously supported execution, not evidence of Guarding execution.

Initial verification used an isolated Dolos copy with local Pallas dependencies.
The dependency correction pins all Pallas Git packages to the published result-API
commit 3ccc547491240e149c7fd9ff648c816292e52020. A fresh public fetch verified its
result API. Canonical-pin verification results are recorded in MUSASHI_FIX_PLAN.md.
No delivered path overrides are used. Earlier isolated tests alone did not verify
public resolution.
