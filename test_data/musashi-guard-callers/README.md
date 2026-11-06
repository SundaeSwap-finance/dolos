# Guard requirements reach the evaluator

Dolos must not infer that a Dijkstra transaction has no scripts to check merely
because its caller supplied no redeemers. Script guard credentials on the top
body or a child body require discovery in Pallas. Pallas resolves their witnesses,
filters native scripts, and reports missing Plutus redeemers. Key-only transactions
retain the existing shortcut.

The tests in `src/serve/grpc/v1/ledger_oracle_tests/guarding.rs` use the existing
ledger-oracle fixture/state helpers and immutable corpus. They exercise actual
Dolos admission, evaluation, estimation, builtin-call estimation and RPC v1.
The unchanged successful guard records compare exact saved Haskell budgets.

- `ic#900`: top-level Plutus guard, 27,906 memory / 6,937,758 steps.
- `ic#1344`: top guard with child-required datum, 5,929 / 1,118,246.
- `ic#1330`: admission rejects a required Plutus guard without its datum.
- `ic#855`, `ic#869`, `ic#899`: native guards, native required-guard null data,
  and a key-only transaction continue to need no Plutus redeemers.

Negative variants remove the redeemer, script or both; put a guard without its
redeemer in a child; change the guard index; exhaust a declared budget; and name
an absent reference input. Other synthetic controls supply the guard through a
reference input, preserve a nonzero guard index, and execute an explicit-error
V4 program. The missing-top-redeemer regression asserts that body bytes remain
unchanged. The body/state mutations are unsigned phase-two tests and make no
successful admission claim. No transaction is funded, signed or sent to a node.

Before the fix, the missing-redeemer paths return `Ok([])` and RPC v1 reports zero
execution units without errors; admission still rejects `RedeemerMissing`.
The same assertions must instead receive the missing script/redeemer error.
The change does not make phase-two evaluation certify admission or enable child
results in RPC formats that cannot identify their transaction level.

Run on the repository's pinned nightly:

```sh
cargo test --locked --offline -p dolos --lib ledger_oracle_tests::guarding::
cargo test --locked --offline -p dolos-cardano --test dijkstra_ttl
```

[provenance.toml](provenance.toml) pins inputs and explains limits. These are
synthetic ledger-conformance records, not a live Musashi capture. The generating
ledger/tool revision is not recorded in the committed corpus; it is unknown.
No on-chain reproduction of this caller bug is available. Original corpus bytes
and parameters are unchanged. Verbose baseline/fixed logs and retry history are
kept outside Git at the location recorded in `MUSASHI_FIX_PLAN.md`.
