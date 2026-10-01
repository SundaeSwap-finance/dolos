# Dijkstra withdrawal admission state

Offline regression for Dolos protocol 12.0, Musashi magic 164, network ID 0.

```sh
cargo test --offline --locked --test dijkstra_withdrawals --test musashi_submission
python3 test_data/musashi-withdrawals/verify.py
```

The single captured transaction is copied byte-for-byte from Pallas commit
`952167c57fff4433dbdc0fac40e145e8dae32795`, `test_data/musashi-dijkstra-validation/`.
Its body hash is `3a44fbaaf49822fa51d1b4f02818298a183e02d73ec1edb8915b48fd52a89a62`.
It exercises V3 spending, minting and two zero-valued script withdrawals. The
12 outputs are its required spending, reference and collateral context; their
roles are in `mint-batch-inputs.json`. Reference scripts account for most bytes.
No other application transaction or redundant parameter bundle is included.

This example is a RealFi V1.1 mint execution transaction. Its two reward scripts
are the application's protocol validators; the settings reference datum is
application configuration. These names do not describe ledger rules. Pallas's
original payload review found SDK source/version metadata and public ledger data,
with no signing secrets or private journals. The exact payload is unchanged.

The validation slot is 1404987 (epoch 65). `account-history.json` records the two
registrations at slot 1401321 and supports registered zero balances at validation.
This is reconstructed historical state under the frozen Dolos expanded archive's
trust, **not an independent node account-state dump or consensus replay**.
`account-state.json` preserves that limitation. Historical node and builder
revisions are unknown. Current spendability is unknown; nothing is submitted.

The tests reuse the existing genesis and historical epoch-64 parameter files in
`../musashi-native-submission/` through `tests/support/musashi_submission.rs`.
The original full 350-entry V3 model and protocol version are retained.
The replay assembles epoch-65 account state and reuses those epoch-64 parameters;
it does not replay the epoch transition or establish independent epoch-65 parameters. The
provenance manifest hashes both imported data and the reused setup dependencies.
Ledger rules are pinned to cardano-ledger `1587f21a7d1306dc590c2749a5c66232ef66aad0`,
Dijkstra `Rules/Entities.hs::validateWithdrawals`: registered accounts are required;
legacy V3 mode drains balances, while non-Plutus withdrawals may be partial.

## Scope and regression coverage

Dolos supplies Pallas with `rewards_sum - withdrawals_sum`, checked for underflow,
from a registered account's current live epoch. UTxO stake is not spendable rewards.
Absent/deregistered accounts are explicitly unregistered; unreadable, missing live,
stale or inconsistent state returns an error. Admission never writes account state.

After phase one checks original ledger balances and signatures, Dolos checks
remaining balances against Pending, Propagated and Acknowledged withdrawals.
Confirmed withdrawals are already in ledger state and are not deducted twice.
A transaction observed in both queue reads during relay is counted once by hash.
Rollback restores the reservation; dropping releases it. Existing serialized
submission protects concurrent admissions. Pallas's legacy draining check sees
original balances, never a reduced mempool balance. Earlier-era admission is unchanged.

Synthetic cases use deterministic public test-only keys and independent UTxOs;
all modified transaction bodies are signed again. They cover zero/partial/full
withdrawals, overdrafts, corrupt/missing/deregistered accounts, signature rejection,
concurrent conflicts and lifecycle transitions using the production Redb mempool
with an in-memory database. Confirmation/rollback use the real `WithdrawalInc`
apply/undo delta plus explicit mempool notifications, not a full block sync replay.
One synthetic reference-datum mutation makes captured scripts fail phase two,
without changing the original transaction, scripts or their commitments.

The positive capture asserts all five script evaluations succeed through Dolos's
full admission path. It proves this historical offline context, not current node
acceptance, complete Dijkstra support or an application's complete lifecycle.
Earlier-era withdrawal support, budget-estimation RPC behavior, direct deposits,
account intervals and unsupported certificate/script forms remain outside this fix.

Before/after results and exact reproduction commands are in `verification.json`.
