# Batch dependency resolution

Dolos pins published Pallas `8ad18dc5d3ac41dfc22605951cb02206e1aa705c`,
checkpoint `musashi-fix-20261015-subtransaction-dependencies-v3`. All Pallas
workspace requirements and crates.io patches use the same exact revision.
The commit was verified on the upstream `leios-musashi` branch and fetched
through Cargo's configured GitHub source. There is no delivered path override.
The checkpoint tag is local until separately published; Cargo pins the published
commit directly and does not need that tag to fetch it.

The regression is `batch_dependencies` in `tests/musashi_submission.rs`, implemented
in `tests/support/musashi_batch_dependencies.rs`. Run:

```sh
cargo test --locked -p dolos --test musashi_submission
```

The test constructs a deterministic, signed Dijkstra key/ADA batch: a parent
spends 10 ADA and two children each spend 5 ADA, the parent produces 19 ADA and
pays a 1 ADA aggregate fee. Child outputs are empty. Parent reference inputs
are empty in the primary acceptance case. All spending UTxOs already exist in
state; none is produced inside the batch. Each body is signed over its own hash.
Keys are constructed from deterministic test-only seeds.

Additional cases retain the same signed transaction while omitting each parent,
child or parent-reference UTxO from prestate, expecting `InputNotInUTxO` for
spending inputs and `ReferenceInputNotInUTxO` for reference inputs. A positive control with the full prestate prevents unrelated failures from making
these negative tests pass. Repeated child spending and parent/child double
spending must reach `DijkstraInputAlreadySpent`, even though the lookup store
collects dependency references into a set. Duplicate reference enumeration
within/across bodies is covered in Pallas's traversal regression; native child
reference-input validation remains unsupported.

Before the Pallas fix, all three new tests compile and fail at runtime with
`InputNotInUTxO`: the child UTxOs are present in storage but never requested by
`MultiEraTx::requires()`. The fix lets Dolos use its existing loader and native
phase-one validation without adding child spending inputs to parent references.
Admission preserves the original synthetic signed bytes and Dijkstra era in
mempool storage. No production Dolos validation shim is added or removed.

These are **synthetic Dolos admission tests**, not historical replay or proof of
upstream Musashi node compatibility. They reuse the existing protocol-12 test
configuration from `tests/support/musashi_submission.rs`; provenance and hashes
of those parameter/genesis files are already recorded in
`../musashi-native-submission/`. The synthetic batch itself has no network
inclusion or current-spendability claim. No live submission or node comparison
was performed. Pallas's unchanged captured traversal evidence is documented in
its `test_data/subtransaction-dependencies/` at the pinned revision.

The caller audit also observed that pending-child input reservations and
child-output lookups use a separate, body-local mempool path. That gap is outside
this dependency loader fix. Scripted/stateful subtransactions remain unsupported.

`provenance.json` records the source/environment identities and proof limits.
`verification.json` and the baseline/fixed transcripts retain the focused evidence;
full local check logs live in `target/subtransaction-published-adoption/`. The local
`MUSASHI_FIX_PLAN.md` records final gates, known failures and publication status.
