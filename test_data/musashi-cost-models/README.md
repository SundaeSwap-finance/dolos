# Musashi genesis cost-model regression

**After rebasing onto 58d91ebb, this commit adds tests/evidence only.** Upstream
Dolos fc2dd7b2 and Pallas de8177d already preserve the injected V1/V2/V3/V4 models
through era translation. The exact published Musashi bootstrap test passes with
those unchanged upstream implementations and dependency pin. Our duplicate
post-translation override was removed. The test now expects the injected V3 model
after translation, preserving upstream semantics. See upstream-rebase.txt.

The remaining sections describe the original investigation before those upstream
changes. Original native data, source hashes and before/after logs are historical
records, not a claim that current upstream still has the original bug.


Official configuration: https://book.play.dev.cardano.org/adv-musashi.html
The page now identifies rc-2026w43. All five downloaded genesis files match the
local files byte-for-byte. Their advertised genesis hashes match too (Byron uses
canonical JSON; the other four use raw bytes). Network magic is 164 and all
configured hard forks occur at epoch zero. See public-source.json/comparison.json.

Expected initial models: V1=332 entries, V2=332, V3=350; V4 from Dijkstra=251.
The regression compares all values, not merely lengths, through the real fresh
Dolos bootstrap. Baseline 5192df05 with Pallas f3419a3 fails at runtime; the same
test and fixtures pass after the fix, using separate build outputs. Ordinary era
translation remains unchanged. The retained reference transition sources specify
that extraConfig overrides are applied after translation.

Correction to the earlier TTL diagnostic: Pallas did NOT retain the V3 injection
in its legacy model map. It recognized only V1/V2 and truncated their extended
arrays. Exact extra models now have a separate representation, and Dolos applies
it only during forced-era genesis bootstrap. No network-specific table is hardcoded.

Run `cargo +1.97.0 test -p dolos-cardano --features strict --test musashi_cost_models`.
The manifest pins the paired Pallas commit; that commit must be published before
a fresh remote-only build can resolve it. Local validation can use the explicit
Cargo override described in ../musashi-ttl/README.md. See baseline.txt/fixed.txt
and verification.json for historical checks and unrelated suite failures.

## Existing state and observed recovery

Rebuilding alone does not replace stored parameters. On 2026-10-30 the user
rebuilt the corrected pair and replayed into a separate fresh database, preserving
the original one. The new instance reports V1/V2/V3/V4=332/332/350/251, with complete
published overrides matching. The TTL spend recorded in the sibling Pallas
repository (attempt 005) then executed successfully on Musashi.

The retained local-active-parameters.json is the BEFORE capture, not current
state. Initial verification did not restart a node or modify its database.
Current observations are separate in Pallas test_data/musashi-ttl/attempts/005/.
No model values were patched into a live database; chain replay preserves any
legitimate on-chain updates. An independent active upstream parameter query
remains unavailable. Published genesis verifies initial state, not every possible
subsequent governance update or builtin.
