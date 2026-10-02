# Submission session recovery regression

These are **representative signed Musashi transactions**, not the failed RealFi
execution `76bd98f395fee95c8897d5877191e6a2ccbc997d6c5b78c8ee81dde1bb6b67f7`.
The supplied report omits its escaped signed payload; the investigation says the
worker did not persist it. Original terminal scrollback was unavailable. No Rust
byte string was decoded, and no private keys or deployment journals are included.

The two hex files are byte-for-byte copies from `../musashi-native-submission/`:

- `control.mempool.hex`: signed key transfer, hash
  `f4ed3784097149431498b0df6ceaf13b3a531df88838d1f8f39df1269eb54b20`.
- `settings-registration.mempool.hex`: signed RealFi settings credential
  registration, hash
  `da910a9bfbe657724d64b505099010dd47f86fb573aa1d20b4da0367163e94c6`.
  “Settings” names RealFi's application configuration script credential; this
  transaction registers it, rather than executing the reported mint batch.

`provenance.json` records SHA-256 of decoded bytes and links the original capture
provenance, historical parameters, inputs, certificate state and limitations.
The existing `tests/support/musashi_submission.rs` helper validates the captures
against that historical state and checks native Dijkstra decoding and signatures.
The recovery tests compare these copies against the admitted transactions, then
check both IDs/sizes/era tags and exact original signed bodies on a recording peer.
The disconnects, block observations and drop threshold of three are synthetic.
This does not establish current ledger validity or full native Dijkstra coverage.

## Recovery behavior

`src/sync/submit.rs` has eight `recovery_*` cases: ephemeral and Redb stores, each
with failure at ID send, body send, after ID delivery before acknowledgement, and
after body delivery and acknowledgement. Send failures cancel the real local
multiplexer after agency transfers; another protocol observes its closed channel
before the tested send. This avoids a race with TCP buffering. Receive failures
close the recording peer. Every Redb case closes and reopens its isolated database
before the replacement connection, exercising the persistent inflight state.

A fresh session queues retained Propagated/Acknowledged hashes for announcement.
It reads current mempool state when servicing requests and never resets the
transaction or its non-confirmation count. IDs may have reached the old peer:
reannouncement is intentional, with the same hash and bytes, and is limited to
once per inherited entry in the new session. Successful sends alone populate the
new positional acknowledgement queue. A protocol acknowledgement is not chain
inclusion. The tests acknowledge one entry at a time, check no duplicate ID
announcements, preserve age, and exclude confirmed entries and entries that drop
while waiting in the recovery queue. Impossible acknowledgement counts restart
the session rather than applying to unrelated entries.

The existing chain-driven drop policy remains in force; reconnect cannot renew
it. There is no new ledger revalidation, wall-clock expiry, or retry timer here.
If chain processing stalls, its block-based aging also stalls. Recovery cannot
resurrect an entry already removed from the inflight store.

The ephemeral store also now acknowledges a Propagated entry after `confirm()`
has moved it into its acknowledgement map, without changing its age or regressing
Confirmed state. Redb already implements that transition.

Diagnostics distinguish ID attempts, body attempts, channel errors with affected
hashes, and new sessions with recovery hashes. gRPC admission logs the accepted
hash instead of dumping the signed request payload.

## Evidence and reproduction

Run `cargo test --locked -p dolos --lib sync::submit::tests -- --nocapture` with
loopback sockets permitted. Tests bind ephemeral ports and use temporary stores;
no running node, live database or network submission is involved.

For the original-code comparison, export Dolos
`b0c772d84923a62b62dda5154c7327d8f56c733d` to a separate directory, replace only
its `src/sync/submit.rs` test module with this commit's module, and copy this
fixture directory. Run the `recovery_` filter with a separate `CARGO_TARGET_DIR`.
No production changes or interface adaptations are needed. The committed
`evidence/` files retain results; complete local logs and reproduction script are
in `target/submission-recovery-checks/`, with baseline sources in
`target/submission-recovery-baseline-source/`.

Final verification: eight identical original-code regressions fail at missing
replacement IDs; all eight and the existing recording-peer test pass with the
fix. Default workspace: **1,846 passed**. Final Dolos library: **141 passed**.
All-features build succeeds. Clippy retains the same **19** baseline warning
locations; workspace formatting retains the original differences in **nine**
untouched files (all four changed Rust files pass). Complete strict-mode testing
retains the same **14** known epoch-coherence failures, with **1,278 passed**.
These existing gates are not claimed clean. See `evidence/verification.json` for
commands, source checksums, comparisons and the final logging follow-up.

## Remaining uncertainty and safe operational verification

The reported logs establish a submission-channel error around the attempt, not
its root cause, exact affected hashes, contract rejection, or Pallas decoder
failure. The fixture demonstrates the recovery defect, not that execution's
actual lifecycle. No Pallas change is required for this fix.

Status reporting is unchanged: `check_status` does not consult the finalized /
dropped log, and gRPC maps Dropped/Unknown to UNSPECIFIED. Neither UNSPECIFIED nor
an admission hash proves rejection, successful relay, or chain inclusion.

Before resuming the existing RealFi batch, obtain deployment authorization and
verify this build in an isolated environment with a controlled upstream reconnect.
Then reconcile the original execution hash against authoritative chain history,
check the original order and every spending/reference/collateral input, transaction
validity and current batch state. Check any other outstanding submissions too.
A production restart with this fix may automatically reannounce still-retained
inflight transactions; reconcile them before deploying. Do not blindly resubmit,
re-sign, or enable workers on the strength of UNSPECIFIED. Investigate the original
upstream disconnect independently and record ID/body/ack/chain-inclusion evidence
before allowing the existing batch to continue.
