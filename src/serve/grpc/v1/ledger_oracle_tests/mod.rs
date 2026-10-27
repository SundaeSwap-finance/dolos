//! Judges what Dolos validates and serves against ledger corpora.

mod params;
mod seed;
mod serve;
mod validate;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

use dolos_core::{BlockHash, ChainPoint, SyncExt as _, TxoRef};
use itertools::Itertools as _;
use ledger_oracle_judge::{
    cbor::{tx_inputs, TxIn},
    compare::{ByteVerdict, StateTallies, Tally},
    corpus::{self, Corpus},
    manifest::{self, Declared, Feature, KnownList, Locus},
    record::{CorpusKind, LedgerOutcome, PParams, Phase2, Record, RecordId, UtxoEntry},
    run::{origin, InputOrigin, KeptRun},
    verdict::{
        outcome, seeds_dependents, with_known, DolosVerdict, Fail, Outcome, ValidationTally,
    },
};
use pallas::ledger::traverse::{Era, MultiEraTx};
use seed::SeedPoint;
use serve::{Event, Served, Service};
use tonic::Request;

use super::{
    query::QueryServiceImpl,
    spec::{
        cardano,
        query::{self as q, query_service_server::QueryService as _},
    },
};
use crate::adapters::DomainAdapter;

fn load(kind: CorpusKind) -> Corpus {
    corpus::load(kind).unwrap_or_else(|e| panic!("{e}"))
}

fn epoch_size(corpus: &Corpus) -> u64 {
    corpus
        .globals
        .as_ref()
        .unwrap_or_else(|| panic!("CORPUS {} has no globals", corpus.kind.name()))
        .epoch_size
}

/// The output bytes Dolos serves through ReadUtxos for each input it holds.
async fn read_utxos<'a>(
    domain: &DomainAdapter,
    inputs: impl IntoIterator<Item = &'a TxIn>,
) -> BTreeMap<TxIn, Vec<u8>> {
    let keys = inputs
        .into_iter()
        .map(|i| q::TxoRef {
            hash: i.tx.to_vec().into(),
            index: seed::txo_ref(i).1,
        })
        .collect();
    let items = QueryServiceImpl::new(domain.clone())
        .read_utxos(Request::new(q::ReadUtxosRequest {
            keys,
            field_mask: None,
        }))
        .await
        .unwrap()
        .into_inner()
        .items;
    items
        .into_iter()
        .map(|item| {
            let r = item.txo_ref.expect("ReadUtxos names each output it serves");
            let input = TxIn {
                tx: r
                    .hash
                    .as_ref()
                    .try_into()
                    .expect("a served hash is 32 bytes"),
                ix: r.index.into(),
            };
            (input, item.native_bytes.to_vec())
        })
        .collect()
}

/// Counts the ledger output, or its absence, at each input and each ledger entry against what Dolos serves there.
async fn compare_utxos<'a>(
    domain: &DomainAdapter,
    id: RecordId,
    inputs: impl IntoIterator<Item = &'a TxIn>,
    expected: &[UtxoEntry],
    tally: &mut Tally,
) {
    let expected: BTreeMap<&TxIn, &[u8]> = expected
        .iter()
        .map(|e| (&e.input, e.output.0.as_slice()))
        .collect();
    let mut keys: BTreeSet<&TxIn> = inputs.into_iter().collect();
    keys.extend(expected.keys());
    let served = read_utxos(domain, keys.iter().copied()).await;
    for input in keys {
        let ledger = expected.get(input).copied();
        let served = served.get(input).map(Vec::as_slice);
        match ByteVerdict::of(ledger, served) {
            None => tally.add(true, String::new),
            Some(verdict) => tally.add(verdict.agrees(), || {
                format!("{id} input={} {verdict}", show_input(input))
            }),
        }
    }
}

fn show_input(input: &TxIn) -> String {
    format!("{}#{}", hex::encode(input.tx), input.ix)
}

/// The Cardano parameters Dolos serves through ReadParams.
async fn read_params(domain: &DomainAdapter) -> cardano::PParams {
    let values = QueryServiceImpl::new(domain.clone())
        .read_params(Request::new(q::ReadParamsRequest { field_mask: None }))
        .await
        .unwrap()
        .into_inner()
        .values
        .expect("ReadParams answers with values");
    match values.params {
        Some(q::any_chain_params::Params::Cardano(p)) => p,
        None => panic!("ReadParams answers with Cardano parameters"),
    }
}

/// The validation outcomes and state comparisons of one corpus.
#[derive(Debug, Default)]
struct Judged {
    validation: ValidationTally,
    state: StateTallies,
}

impl Judged {
    fn pass(&self) -> bool {
        self.validation.pass() && self.state.pass()
    }
}

impl fmt::Display for Judged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.validation, self.state)
    }
}

/// The declared support and the known list a judgement reads.
#[derive(Debug, Clone)]
struct Manifests {
    declared: Declared,
    known: KnownList,
}

impl Manifests {
    fn committed() -> Self {
        Manifests {
            declared: manifest::declared().unwrap_or_else(|e| panic!("{e}")),
            known: manifest::known().unwrap_or_else(|e| panic!("{e}")),
        }
    }

    /// Fails each declared or validation known entry for the corpus that names a record the judgement did not reach.
    fn unjudged(&self, kind: CorpusKind, judged: &BTreeSet<RecordId>, tally: &mut ValidationTally) {
        let missed = |id: &RecordId| id.corpus == kind && !judged.contains(id);
        for (id, feature) in self.declared.iter().filter(|(id, _)| missed(id)) {
            let fail = Fail::StaleManifest {
                declared: feature.to_string(),
                dolos: "not_judged".to_string(),
            };
            tally.add(*id, Outcome::Fail(fail));
        }
        let validation = |locus: &Locus| *locus == Locus::Validation;
        let entries = self.known.iter();
        for ((_, id), row) in entries.filter(|((locus, id), _)| validation(locus) && missed(id)) {
            let fail = Fail::KnownStale { row: row.clone() };
            tally.add(*id, Outcome::Fail(fail));
        }
    }
}

/// Every input a record names, at the top level and in its sub transactions.
fn named_inputs(id: RecordId, record: &Record) -> BTreeSet<TxIn> {
    let inputs = tx_inputs(&record.tx.0).unwrap_or_else(|e| panic!("TX {id} {e}"));
    let subs = inputs
        .subs
        .iter()
        .flat_map(|s| s.spend.iter().chain(&s.reference));
    inputs
        .spend
        .iter()
        .chain(&inputs.collateral)
        .chain(&inputs.reference)
        .chain(subs)
        .copied()
        .collect()
}

/// The pre state entries for the inputs of a record that Dolos misses because a maker in its group was not taken.
fn missing_from_makers<'r>(
    group: &validate::Group,
    record: &'r Record,
    inputs: &BTreeSet<TxIn>,
    makers: &BTreeMap<TxIn, RecordId>,
    records: &[Record],
    seeding: &BTreeSet<RecordId>,
) -> (Vec<&'r UtxoEntry>, BTreeSet<RecordId>) {
    let mut entries = Vec::new();
    let mut made_by = BTreeSet::new();
    for input in inputs {
        let maker = match origin(input, record, makers, records) {
            InputOrigin::InGroup { made_by } => made_by,
            InputOrigin::Seeded | InputOrigin::InRun { .. } | InputOrigin::AbsentFromPre => {
                continue
            }
        };
        if !seeding.contains(&maker) || group.sees(seed::txo_ref(input)) {
            continue;
        }
        let entry = record.pre_utxo.iter().find(|e| e.input == *input);
        entries.push(entry.expect("an in group input is in the pre state"));
        made_by.insert(maker);
    }
    (entries, made_by)
}

/// A block a run rolled, with the records it holds and their slot group.
struct Rolled<'r> {
    point: ChainPoint,
    group: &'r [Record],
    records: Vec<&'r Record>,
}

/// Submits each slot group of a run in order to a fresh mempool at the group slot, then rolls the block of its accepted records, compares the outputs and returns the blocks it rolled.
async fn validate_run<'r>(
    base: &DomainAdapter,
    kind: CorpusKind,
    run: &'r KeptRun,
    manifests: &Manifests,
    edits: &Edits<'_>,
    judged: &mut BTreeSet<RecordId>,
    tallies: &mut Judged,
) -> Vec<Rolled<'r>> {
    let Judged {
        validation: tally,
        state,
    } = tallies;
    let id_of = |record: &Record| RecordId {
        corpus: kind,
        n: record.n,
    };
    let makers = run.makers(kind);
    let mut tip = None;
    let mut rolled = Vec::new();
    for records in run.groups() {
        let group = validate::group(base, records[0].slot);
        let mut seeding = BTreeSet::new();
        for record in records {
            let id = id_of(record);
            let inputs = named_inputs(id, record);
            let (entries, made_by) =
                missing_from_makers(&group, record, &inputs, &makers, records, &seeding);
            if !entries.is_empty() {
                group.seed(&entries);
                tally.dependent.push((id, made_by));
            }
            let dolos = validate::verdict(&group.receive(&record.tx.0));
            let declared = manifests.declared.get(&id);
            let judgement = outcome(&record.outcome, record.phase2_valid, &dolos, declared);
            let known = manifests.known.get(&(Locus::Validation, id));
            let judgement = with_known(judgement, known);
            if seeds_dependents(&dolos, &judgement) {
                seeding.insert(id);
            }
            tally.add(id, judgement);
            judged.insert(id);
        }
        let accepted = records.iter().filter_map(|r| match &r.outcome {
            LedgerOutcome::Accepted { post_utxo, .. } => Some((r, post_utxo)),
            LedgerOutcome::Refused { .. } => None,
        });
        let Some((last, post)) = accepted.clone().next_back() else {
            continue;
        };
        let included: Vec<(&Record, Phase2)> = accepted
            .filter_map(|(r, _)| Some((r, (edits.block_tx)(id_of(r), r.phase2_valid)?)))
            .collect();
        let txs: Vec<_> = included
            .iter()
            .map(|(r, phase2)| (r.tx.0.as_slice(), *phase2))
            .collect();
        let (next, block) = validate::block(last.slot, tip, &txs);
        if let Err(e) = group.roll(block) {
            let id = id_of(last);
            state.utxo.add(false, || format!("{id} roll_forward {e}"));
            // A failed block leaves the tip where it was, so the next block names that parent.
            continue;
        }
        tip = Some(next);
        rolled.push(Rolled {
            point: ChainPoint::Specific(last.slot, next.hash()),
            group: records,
            records: included.into_iter().map(|(r, _)| r).collect(),
        });
        let inputs = records.iter().flat_map(|r| &r.pre_utxo).map(|e| &e.input);
        compare_utxos(group.domain(), id_of(last), inputs, post, &mut state.utxo).await;
    }
    rolled
}

/// The output bytes each service serves for the blocks a run rolled, streamed from the seed point.
async fn serve_rolled(
    domain: &DomainAdapter,
    seed: &SeedPoint,
    rolled: &[Rolled<'_>],
) -> [(Service, Served); 3] {
    let hashes: Vec<BlockHash> = rolled
        .iter()
        .map(|r| r.point.hash().expect("a rolled block has a hash"))
        .collect();
    let txs = rolled.iter().map(|r| r.records.len()).sum();
    let streams = serve::Streams::open(domain, seed.point()).await;
    let [follow, watch] = streams.read(Event::Apply, hashes.len(), txs).await;
    let fetch = serve::fetch_block(domain, &hashes).await;
    [(Service::FetchBlock, fetch), follow, watch]
}

/// What each service serves for one input against the ledger's output bytes, grouping the services by each disagreeing verdict.
fn served_verdicts(
    tx: &[u8; 32],
    input: &TxIn,
    ledger: &[u8],
    services: &[(Service, Served)],
) -> BTreeMap<String, BTreeSet<Service>> {
    let mut failures: BTreeMap<String, BTreeSet<Service>> = BTreeMap::new();
    for (service, served) in services {
        let verdicts: Vec<String> = match served.get(tx) {
            None => vec!["not_served".to_string()],
            Some(named) => {
                let occurrences: Vec<_> = named.iter().filter(|(i, _)| i == input).collect();
                if occurrences.is_empty() {
                    vec!["not_named".to_string()]
                } else {
                    occurrences
                        .into_iter()
                        .filter_map(|(_, output)| ByteVerdict::of(Some(ledger), output.as_deref()))
                        .filter(|v| !v.agrees())
                        .map(|v| v.to_string())
                        .collect()
                }
            }
        };
        for verdict in verdicts {
            failures.entry(verdict).or_default().insert(*service);
        }
    }
    failures
}

/// The served output comparisons of the records of one block, with the inputs left uncompared because the run's seed made them and the compared inputs only sub transactions name.
#[derive(Debug, Default)]
struct ServedChecks {
    records: BTreeMap<RecordId, Tally>,
    seeded: usize,
    sub_only: usize,
}

/// Counts each input a record of the block names that an earlier record made, against what every service serves for it.
fn compare_served(
    kind: CorpusKind,
    makers: &BTreeMap<TxIn, RecordId>,
    block: &Rolled<'_>,
    services: &[(Service, Served)],
) -> ServedChecks {
    let mut checks = ServedChecks::default();
    for record in &block.records {
        let id = RecordId {
            corpus: kind,
            n: record.n,
        };
        let parsed = tx_inputs(&record.tx.0).unwrap_or_else(|e| panic!("TX {id} {e}"));
        let sub_only = parsed.sub_only();
        let tx = MultiEraTx::decode_for_era(Era::Dijkstra, &record.tx.0)
            .unwrap_or_else(|e| panic!("TX {id} {e}"));
        let hash: [u8; 32] = *tx.hash();
        let tally = checks.records.entry(id).or_default();
        for input in named_inputs(id, record) {
            let shown = show_input(&input);
            let made = match origin(&input, record, makers, block.group) {
                InputOrigin::Seeded => {
                    checks.seeded += 1;
                    continue;
                }
                InputOrigin::AbsentFromPre => {
                    tally.add(false, || format!("absent_from_pre input={shown}"));
                    continue;
                }
                InputOrigin::InRun { made_by } => format!("in_run made_by={made_by}"),
                InputOrigin::InGroup { made_by } => format!("in_group made_by={made_by}"),
            };
            if sub_only.contains(&input) {
                checks.sub_only += 1;
            }
            let entry = record.pre_utxo.iter().find(|e| e.input == input);
            let ledger = &entry
                .expect("an input made in the run is in the pre state")
                .output;
            let failures = served_verdicts(&hash, &input, &ledger.0, services);
            tally.add(failures.is_empty(), || {
                let lines = failures.iter().map(|(verdict, services)| {
                    let mut names = services.iter().map(Service::to_string);
                    let names = names.join(",");
                    format!("{verdict} input={shown} {made} services={names}")
                });
                lines.collect::<Vec<_>>().join(" and ")
            });
        }
    }
    checks
}

/// Counts the served outputs of every rolled block into the per record as_output checks.
fn compare_as_output(
    kind: CorpusKind,
    run: &KeptRun,
    rolled: &[Rolled<'_>],
    services: &[(Service, Served)],
    state: &mut StateTallies,
    checks: &mut BTreeMap<RecordId, Tally>,
) {
    let makers = run.makers(kind);
    for block in rolled {
        let served = compare_served(kind, &makers, block, services);
        state.seeded_unjudged += served.seeded;
        state.in_run += served.sub_only;
        for (id, tally) in served.records {
            let merged = checks.entry(id).or_default();
            merged.compared += tally.compared;
            merged.disagree.extend(tally.disagree);
        }
    }
}

/// Rolls a run back one block at a time to its seed point, counting what FollowTip and WatchTx serve for each undone input, what ReadUtxos serves for the inputs earlier blocks made, and the seed outputs it serves at the end.
async fn sweep(
    domain: &DomainAdapter,
    seed: &SeedPoint,
    kind: CorpusKind,
    run: &KeptRun,
    rolled: &[Rolled<'_>],
    tally: &mut Tally,
) {
    let makers = run.makers(kind);
    let first = &run.records()[0];
    let id = RecordId {
        corpus: kind,
        n: first.n,
    };
    for (i, block) in rolled.iter().enumerate().rev() {
        let to = match i.checked_sub(1) {
            Some(previous) => &rolled[previous].point,
            None => seed.point(),
        };
        let streams = serve::Streams::open(domain, &block.point).await;
        if let Err(e) = domain.rollback(to) {
            tally.add(false, || format!("{id} rollback to={to} {e}"));
            return;
        }
        let services = streams.read(Event::Undo, 1, block.records.len()).await;
        for (record, checked) in compare_served(kind, &makers, block, &services).records {
            tally.compared += checked.compared;
            let lines = checked.disagree.into_iter();
            tally
                .disagree
                .extend(lines.map(|d| format!("{record} {d}")));
        }
        for record in &block.records {
            let rid = RecordId {
                corpus: kind,
                n: record.n,
            };
            let restored: Vec<UtxoEntry> = record
                .pre_utxo
                .iter()
                .filter(|e| {
                    let named = named_inputs(rid, record).contains(&e.input);
                    let from = origin(&e.input, record, &makers, block.group);
                    named && matches!(from, InputOrigin::InRun { .. })
                })
                .cloned()
                .collect();
            compare_utxos(domain, rid, [], &restored, tally).await;
        }
    }
    compare_utxos(domain, id, [], &first.pre_utxo, tally).await;
}

/// What a planted check changes in the seed, the expected parameters and the blocks of one judgement.
struct Edits<'a> {
    pre: Box<dyn Fn(RecordId, &[UtxoEntry]) -> Vec<UtxoEntry> + 'a>,
    expected_params: Box<dyn Fn(&PParams) -> PParams + 'a>,
    block_tx: Box<dyn Fn(RecordId, Phase2) -> Option<Phase2> + 'a>,
}

impl Default for Edits<'_> {
    fn default() -> Self {
        Edits {
            pre: Box::new(|_, pre| pre.to_vec()),
            expected_params: Box::new(PParams::clone),
            block_tx: Box::new(|_, phase2| Some(phase2)),
        }
    }
}

/// Judges every kept run of a corpus, each seeded from its first pre state.
async fn judge(corpus: &Corpus, manifests: &Manifests, edits: &Edits<'_>) -> Judged {
    let genesis = seed::genesis(epoch_size(corpus));
    let mut judged = Judged::default();
    let mut judged_ids = BTreeSet::new();
    let mut as_output = BTreeMap::new();
    for run in corpus.kept() {
        let first = &run.records()[0];
        let id = RecordId {
            corpus: corpus.kind,
            n: first.n,
        };
        let epoch = run
            .epoch()
            .unwrap_or_else(|| panic!("RUN {id} has no epoch"));
        let pparams = params::seed_values(run.pparams()).unwrap_or_else(|e| panic!("{e}"));
        let seeded = seed::seed(&genesis, epoch, &pparams, &(edits.pre)(id, &first.pre_utxo));
        compare_utxos(
            &seeded.domain,
            id,
            [],
            &first.pre_utxo,
            &mut judged.state.utxo,
        )
        .await;
        let served = read_params(&seeded.domain).await;
        let expected = (edits.expected_params)(run.pparams());
        params::compare(id, &expected, &served, &mut judged.state.params);
        let rolled = validate_run(
            &seeded.domain,
            corpus.kind,
            run,
            manifests,
            edits,
            &mut judged_ids,
            &mut judged,
        )
        .await;
        let services = serve_rolled(&seeded.domain, &seeded.seed, &rolled).await;
        compare_as_output(
            corpus.kind,
            run,
            &rolled,
            &services,
            &mut judged.state,
            &mut as_output,
        );
        sweep(
            &seeded.domain,
            &seeded.seed,
            corpus.kind,
            run,
            &rolled,
            &mut judged.state.undo,
        )
        .await;
    }
    manifests.unjudged(corpus.kind, &judged_ids, &mut judged.validation);
    judged
        .state
        .settle_as_output(corpus.kind, as_output, &manifests.known);
    judged
}

/// Judges the supplement corpus under edited manifests.
async fn judge_supp(edit: impl FnOnce(&mut Manifests)) -> Judged {
    let mut manifests = Manifests::committed();
    edit(&mut manifests);
    judge(&load(CorpusKind::Supp), &manifests, &Edits::default()).await
}

fn supp(n: u32) -> RecordId {
    RecordId {
        corpus: CorpusKind::Supp,
        n,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn ledger_oracle() {
    let mut pass = true;
    let manifests = Manifests::committed();
    for kind in [CorpusKind::Ic, CorpusKind::Supp] {
        let corpus = load(kind);
        println!("{corpus}");
        let tallies = judge(&corpus, &manifests, &Edits::default()).await;
        println!("{tallies}");
        pass &= tallies.pass();
    }
    let result = if pass { "PASS" } else { "FAIL" };
    println!("RESULT {result}");
    assert!(pass);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn dropping_one_seed_pair_prints_utxo_disagree() {
    let corpus = load(CorpusKind::Supp);
    let manifests = Manifests::committed();
    let intact = judge(&corpus, &manifests, &Edits::default()).await;
    assert!(intact.state.utxo.compared > 0);
    assert!(intact.state.utxo.disagree.is_empty(), "{intact}");
    let dropped = AtomicBool::new(false);
    let edits = Edits {
        pre: Box::new(|_, pre| {
            let skip = !dropped.swap(true, Ordering::Relaxed);
            pre.iter().skip(usize::from(skip)).cloned().collect()
        }),
        ..Edits::default()
    };
    let planted = judge(&corpus, &manifests, &edits).await;
    let first = &corpus.kept().next().unwrap().records()[0];
    let pair = &first.pre_utxo[0];
    let line = format!(
        "{} input={} ledger={} dolos=none",
        supp(first.n),
        show_input(&pair.input),
        hex::encode(&pair.output.0)
    );
    assert!(planted.state.utxo.disagree.contains(&line), "{planted}");
    assert!(planted.to_string().contains("\nSTATE utxo disagree="));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn one_changed_cost_model_entry_prints_params_disagree() {
    let corpus = load(CorpusKind::Supp);
    let manifests = Manifests::committed();
    let intact = judge(&corpus, &manifests, &Edits::default()).await;
    assert!(intact.state.params.compared > 0);
    assert!(intact.state.params.disagree.is_empty(), "{intact}");
    let changed = AtomicBool::new(false);
    let edits = Edits {
        expected_params: Box::new(|pparams| {
            let mut pparams = pparams.clone();
            if !changed.swap(true, Ordering::Relaxed) {
                let entry = &mut pparams.0["costModels"]["PlutusV1"][0];
                *entry = (entry.as_i64().unwrap() + 1).into();
            }
            pparams
        }),
        ..Edits::default()
    };
    let planted = judge(&corpus, &manifests, &edits).await;
    let text = planted.to_string();
    assert!(text.contains("STATE params disagree=1\n"), "{text}");
    assert!(text.contains("costModels.PlutusV1[0] ledger="), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn the_v2_record_is_known_and_its_maker_agrees() {
    let text = judge_supp(|_| ()).await.validation.to_string();
    assert!(!text.contains(" supp#1 "), "{text}");
    assert!(!text.contains("\nDISAGREE supp#2 "), "{text}");
    assert!(
        text.contains("\nKNOWN row=V2 ids=supp#2 ledger=accepted dolos=refused[InputNotInUTxO]"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn deleting_the_v2_known_entry_prints_its_disagreement() {
    let text = judge_supp(|m| {
        m.known.remove(&(Locus::Validation, supp(2))).unwrap();
    })
    .await
    .to_string();
    assert!(!text.contains("\nKNOWN row=V2 "), "{text}");
    assert!(
        text.contains("\nDISAGREE supp#2 ledger=accepted dolos=refused[InputNotInUTxO]"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn deleting_one_declared_entry_prints_unsupported_unexpected() {
    let label = "stateful or scripted subtransaction";
    let intact = judge_supp(|_| ()).await.to_string();
    assert!(
        !intact.contains("UNSUPPORTED_UNEXPECTED supp#14 "),
        "{intact}"
    );
    let deleted = judge_supp(|m| {
        assert_eq!(m.declared.remove(&supp(14)), Some(Feature(label.into())));
        m.declared.insert(supp(999), Feature(label.into()));
    })
    .await
    .to_string();
    assert!(
        deleted.contains(&format!("\nUNSUPPORTED_UNEXPECTED supp#14 label={label}")),
        "{deleted}"
    );
    assert!(
        deleted.contains(&format!(
            "\nSTALE_MANIFEST supp#999 declared={label} dolos=not_judged"
        )),
        "{deleted}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn a_known_entry_naming_an_agreeing_or_unjudged_record_prints_known_stale() {
    let intact = judge_supp(|_| ()).await.validation.to_string();
    assert!(!intact.contains("supp#0 "), "{intact}");
    let named = judge_supp(|m| {
        m.known
            .insert((Locus::Validation, supp(0)), "V9".to_string());
        m.known
            .insert((Locus::Validation, supp(999)), "V9".to_string());
    })
    .await
    .to_string();
    assert!(named.contains("\nKNOWN_STALE supp#0 row=V9"), "{named}");
    assert!(named.contains("\nKNOWN_STALE supp#999 row=V9"), "{named}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn a_maker_that_fails_leaves_its_dependents_disagreeing() {
    let corpus = load(CorpusKind::Ic);
    let committed = Manifests::committed();
    let intact = judge(&corpus, &committed, &Edits::default()).await;
    let maker = intact
        .validation
        .dependent
        .iter()
        .flat_map(|(_, makers)| makers)
        .find(|m| committed.declared.contains_key(m))
        .copied()
        .expect("a declared unsupported maker has a seeded dependent");
    let dependents: BTreeSet<RecordId> = intact
        .validation
        .dependent
        .iter()
        .filter(|(_, makers)| makers.contains(&maker))
        .map(|(id, _)| *id)
        .collect();
    let disagrees = |judged: &Judged| -> BTreeSet<RecordId> {
        let ids = judged.validation.disagree.iter().map(|(id, _)| *id);
        ids.filter(|id| dependents.contains(id)).collect()
    };
    assert!(disagrees(&intact).is_empty(), "{intact}");
    let mut failing = committed.clone();
    failing.declared.remove(&maker).unwrap();
    let patched = judge(&corpus, &failing, &Edits::default()).await;
    assert!(
        patched
            .validation
            .dependent
            .iter()
            .all(|(_, makers)| !makers.contains(&maker)),
        "{patched}"
    );
    assert!(
        !disagrees(&patched).is_empty(),
        "maker={maker} dependents={dependents:?} {patched}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn dropping_the_seeded_input_under_a_metadata_refusal_prints_reason_mismatch() {
    let corpus = load(CorpusKind::Ic);
    let manifests = Manifests::committed();
    let run = corpus
        .kept()
        .find(|r| r.records().iter().any(|x| x.n == 1311))
        .unwrap();
    let first = &run.records()[0];
    assert_eq!(first.pre_utxo.len(), 1, "the run is seeded with one output");
    let first = RecordId {
        corpus: CorpusKind::Ic,
        n: first.n,
    };
    let line = "\nREASON_MISMATCH ic#1311 ";
    let intact = judge(&corpus, &manifests, &Edits::default()).await;
    assert!(!intact.to_string().contains(line), "{intact}");
    let edits = Edits {
        pre: Box::new(|id, pre| if id == first { vec![] } else { pre.to_vec() }),
        ..Edits::default()
    };
    let planted = judge(&corpus, &manifests, &edits).await.to_string();
    assert!(
        planted.contains(&format!(
            "{line}ledger=refused[SubMissingTxMetadata] ledger_class=metadata \
             dolos=refused[InputNotInUTxO] dolos_class=inputs"
        )),
        "{planted}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn deleting_the_v1_known_entry_prints_its_reason_mismatch() {
    let intact = judge_supp(|_| ()).await.to_string();
    assert!(intact.contains("\nKNOWN row=V1 ids=supp#5 "), "{intact}");
    let deleted = judge_supp(|m| {
        m.known.remove(&(Locus::Validation, supp(5))).unwrap();
    })
    .await
    .to_string();
    assert!(
        deleted.contains(
            "\nREASON_MISMATCH supp#5 ledger=refused[BadInputsUTxO,BadInputsUTxO] \
             ledger_class=inputs dolos=refused[PreservationOfValue] dolos_class=value"
        ),
        "{deleted}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn leaving_an_accepted_tx_out_of_its_block_prints_utxo_disagree() {
    let corpus = load(CorpusKind::Supp);
    let manifests = Manifests::committed();
    let left_out = corpus
        .kept()
        .flat_map(|run| run.groups().filter_map(<[Record]>::last))
        .find(|r| matches!(r.outcome, LedgerOutcome::Accepted { .. }))
        .map(|r| supp(r.n))
        .expect("a group ends in an accepted record");
    let missing = |judged: &Judged| {
        let prefix = format!("{left_out} input=");
        let lines = judged.state.utxo.disagree.iter();
        lines
            .filter(|d| d.starts_with(&prefix) && d.ends_with(" dolos=none"))
            .count()
    };
    let intact = judge(&corpus, &manifests, &Edits::default()).await;
    assert_eq!(missing(&intact), 0, "{intact}");
    let edits = Edits {
        block_tx: Box::new(|id, phase2| (id != left_out).then_some(phase2)),
        ..Edits::default()
    };
    let planted = judge(&corpus, &manifests, &edits).await;
    assert!(missing(&planted) > 0, "{left_out} {planted}");
    assert!(planted.to_string().contains("\nSTATE utxo disagree="));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn supp8_written_as_phase2_valid_leaves_the_input_of_supp9_missing() {
    let corpus = load(CorpusKind::Supp);
    let supp8 = corpus
        .kept()
        .flat_map(KeptRun::records)
        .find(|r| r.n == 8)
        .expect("supp#8 is in a kept run");
    assert_eq!(supp8.phase2_valid, Phase2::Invalid);
    assert!(matches!(supp8.outcome, LedgerOutcome::Accepted { .. }));
    let manifests = Manifests::committed();
    let line = "\nDISAGREE supp#9 ";
    let intact = judge(&corpus, &manifests, &Edits::default()).await;
    assert!(!intact.to_string().contains(line), "{intact}");
    let edits = Edits {
        block_tx: Box::new(|id, phase2| match id == supp(8) {
            true => Some(Phase2::Valid),
            false => Some(phase2),
        }),
        ..Edits::default()
    };
    let planted = judge(&corpus, &manifests, &edits).await.to_string();
    assert!(
        planted.contains(&format!(
            "{line}ledger=accepted dolos=refused[InputNotInUTxO]"
        )),
        "{planted}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn a_tx_received_in_one_group_is_absent_from_the_next() {
    let corpus = load(CorpusKind::Supp);
    let genesis = seed::genesis(epoch_size(&corpus));
    let mut checked = 0;
    for run in corpus.kept() {
        let groups: Vec<_> = run.groups().collect();
        let [first, second, ..] = groups.as_slice() else {
            continue;
        };
        let pparams = params::seed_values(run.pparams()).unwrap();
        let seeded = seed::seed(
            &genesis,
            run.epoch().unwrap(),
            &pparams,
            &run.records()[0].pre_utxo,
        );
        let one = validate::group(&seeded.domain, first[0].slot);
        let Some(hash) = first
            .iter()
            .filter(|r| matches!(r.outcome, LedgerOutcome::Accepted { .. }))
            .find_map(|r| one.receive(&r.tx.0).ok())
        else {
            continue;
        };
        let made = TxoRef(hash, 0);
        assert!(one.sees(made.clone()));
        let two = validate::group(&seeded.domain, second[0].slot);
        assert!(!two.sees(made));
        checked += 1;
    }
    assert!(checked > 0);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn the_collateral_return_supp9_spends_is_known_unserved() {
    let line = "\nKNOWN row=S3 ids=supp#9 as_output ledger=";
    let intact = judge_supp(|_| ()).await;
    let text = intact.to_string();
    assert!(text.contains(line), "{text}");
    let known = text.lines().find(|l| l.starts_with(&line[1..])).unwrap();
    assert!(known.contains(" dolos=none input="), "{known}");
    for service in ["FetchBlock", "FollowTip", "WatchTx"] {
        assert!(known.contains(service), "{known}");
    }
    assert!(intact.state.as_output.disagree.is_empty(), "{text}");
    assert!(intact.state.as_output.compared > 0, "{text}");
    let deleted = judge_supp(|m| {
        m.known.remove(&(Locus::AsOutput, supp(9))).unwrap();
    })
    .await
    .to_string();
    assert!(!deleted.contains("\nKNOWN row=S3 "), "{deleted}");
    assert!(deleted.contains("\nSTATE as_output disagree="), "{deleted}");
    assert!(
        deleted.contains("\nDIFF as_output supp#9 ledger="),
        "{deleted}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn rolling_back_each_block_serves_the_inputs_earlier_blocks_made() {
    let corpus = load(CorpusKind::Supp);
    let seeds: usize = corpus.kept().map(|r| r.records()[0].pre_utxo.len()).sum();
    let judged = judge(&corpus, &Manifests::committed(), &Edits::default()).await;
    let text = judged.to_string();
    assert!(judged.state.undo.compared > seeds, "seeds={seeds} {text}");
    let other = judged
        .state
        .undo
        .disagree
        .iter()
        .filter(|d| !d.contains(" in_group made_by="))
        .join("\n");
    assert!(other.is_empty(), "{text}\nUNDO\n{other}");
}

#[test]
fn a_dolos_refusal_is_named_by_its_kind() {
    let refused: Result<dolos_core::TxHash, _> = Err(dolos_core::DomainError::ChainError(
        dolos_core::ChainError::Phase1ValidationRejected(
            pallas::ledger::validate::utils::ValidationError::PostAlonzo(
                pallas::ledger::validate::utils::PostAlonzoError::InputNotInUTxO,
            ),
        ),
    ));
    assert_eq!(
        validate::verdict(&refused),
        DolosVerdict::Refused("InputNotInUTxO".to_string())
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn a_seed_output_compared_as_bytes_names_the_first_differing_byte() {
    let corpus = load(CorpusKind::Supp);
    let run = corpus.kept().next().unwrap();
    let first = &run.records()[0];
    let genesis = seed::genesis(epoch_size(&corpus));
    let seeded = seed::seed(&genesis, run.epoch().unwrap(), &[], &first.pre_utxo);
    let id = RecordId {
        corpus: corpus.kind,
        n: first.n,
    };
    let mut equal = Tally::default();
    compare_utxos(&seeded.domain, id, [], &first.pre_utxo[..1], &mut equal).await;
    assert_eq!((equal.compared, equal.disagree.len()), (1, 0));
    let mut flipped = first.pre_utxo[..1].to_vec();
    flipped[0].output.0[3] ^= 1;
    let mut differ = Tally::default();
    compare_utxos(&seeded.domain, id, [], &flipped, &mut differ).await;
    assert_eq!(differ.disagree.len(), 1);
    assert!(
        differ.disagree[0].ends_with("differ at byte 3"),
        "{}",
        differ.disagree[0]
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn the_seed_point_is_a_rollback_target() {
    let corpus = load(CorpusKind::Supp);
    let run = corpus.kept().next().unwrap();
    let genesis = seed::genesis(epoch_size(&corpus));
    let seeded = seed::seed(
        &genesis,
        run.epoch().unwrap(),
        &[],
        &run.records()[0].pre_utxo,
    );
    let rolled = seeded.domain.rollback(seeded.seed.point());
    assert!(rolled.is_ok(), "{rolled:?}");
}
