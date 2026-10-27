//! Judges what Dolos validates and serves against ledger corpora.

mod params;
mod seed;
mod validate;

use std::{
    collections::BTreeMap,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

use dolos_core::{SyncExt as _, TxoRef};
use ledger_oracle_judge::{
    cbor::TxIn,
    compare::{ByteVerdict, StateTallies, Tally},
    corpus::{self, Corpus},
    record::{CorpusKind, LedgerOutcome, PParams, RecordId, UtxoEntry},
    run::KeptRun,
    verdict::{outcome, DolosVerdict, ValidationTally},
};
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

/// Counts each ledger entry against the output bytes Dolos serves for its input.
async fn compare_utxos(
    domain: &DomainAdapter,
    id: RecordId,
    expected: &[UtxoEntry],
    tally: &mut Tally,
) {
    let served = read_utxos(domain, expected.iter().map(|e| &e.input)).await;
    for e in expected {
        let served = served.get(&e.input).map(Vec::as_slice);
        let verdict = ByteVerdict::of(Some(e.output.0.as_slice()), served)
            .expect("the ledger side is present");
        tally.add(verdict.agrees(), || {
            format!("{id} input={} {verdict}", show_input(&e.input))
        });
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

/// Submits each slot group of a run in order, each to a fresh mempool at the group slot.
fn validate_run(base: &DomainAdapter, kind: CorpusKind, run: &KeptRun, tally: &mut ValidationTally) {
    for records in run.groups() {
        let group = validate::group(base, records[0].slot);
        for record in records {
            let id = RecordId {
                corpus: kind,
                n: record.n,
            };
            let dolos = validate::verdict(&group.receive(&record.tx.0));
            tally.add(id, outcome(&record.outcome, record.phase2_valid, &dolos));
        }
    }
}

/// Judges every kept run of a corpus, each seeded from its first pre state.
async fn judge(
    corpus: &Corpus,
    edit_pre: impl Fn(&[UtxoEntry]) -> Vec<UtxoEntry>,
    edit_expected_params: impl Fn(&PParams) -> PParams,
) -> Judged {
    let genesis = seed::genesis(epoch_size(corpus));
    let mut judged = Judged::default();
    let tallies = &mut judged.state;
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
        let seeded = seed::seed(&genesis, epoch, &pparams, &edit_pre(&first.pre_utxo));
        compare_utxos(&seeded.domain, id, &first.pre_utxo, &mut tallies.utxo).await;
        let served = read_params(&seeded.domain).await;
        let expected = edit_expected_params(run.pparams());
        params::compare(id, &expected, &served, &mut tallies.params);
        validate_run(&seeded.domain, corpus.kind, run, &mut judged.validation);
    }
    judged
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn ledger_oracle() {
    let mut pass = true;
    for kind in [CorpusKind::Ic, CorpusKind::Supp] {
        let corpus = load(kind);
        println!("{corpus}");
        let tallies = judge(&corpus, <[UtxoEntry]>::to_vec, PParams::clone).await;
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
    let intact = judge(&corpus, <[UtxoEntry]>::to_vec, PParams::clone).await;
    assert!(intact.state.utxo.compared > 0);
    assert!(intact.state.utxo.disagree.is_empty(), "{intact}");
    let dropped = AtomicBool::new(false);
    let planted = judge(
        &corpus,
        |pre| {
            let skip = !dropped.swap(true, Ordering::Relaxed);
            pre.iter().skip(usize::from(skip)).cloned().collect()
        },
        PParams::clone,
    )
    .await;
    let text = planted.to_string();
    assert!(text.contains("STATE utxo disagree=1\n"), "{text}");
    assert!(text.contains("ledger="), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn one_changed_cost_model_entry_prints_params_disagree() {
    let corpus = load(CorpusKind::Supp);
    let intact = judge(&corpus, <[UtxoEntry]>::to_vec, PParams::clone).await;
    assert!(intact.state.params.compared > 0);
    assert!(intact.state.params.disagree.is_empty(), "{intact}");
    let changed = AtomicBool::new(false);
    let planted = judge(&corpus, <[UtxoEntry]>::to_vec, |pparams| {
        let mut pparams = pparams.clone();
        if !changed.swap(true, Ordering::Relaxed) {
            let entry = &mut pparams.0["costModels"]["PlutusV1"][0];
            *entry = (entry.as_i64().unwrap() + 1).into();
        }
        pparams
    })
    .await;
    let text = planted.to_string();
    assert!(text.contains("STATE params disagree=1\n"), "{text}");
    assert!(text.contains("costModels.PlutusV1[0] ledger="), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "judges Dolos against the committed ledger corpora, run in release"]
async fn the_v2_record_disagrees_and_its_maker_agrees() {
    let judged = judge(
        &load(CorpusKind::Supp),
        <[UtxoEntry]>::to_vec,
        PParams::clone,
    )
    .await;
    let text = judged.to_string();
    assert!(!text.contains(" supp#1 "), "{text}");
    assert!(
        text.contains("\nDISAGREE supp#2 ledger=accepted dolos=refused[InputNotInUTxO]"),
        "{text}"
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
    compare_utxos(&seeded.domain, id, &first.pre_utxo[..1], &mut equal).await;
    assert_eq!((equal.compared, equal.disagree.len()), (1, 0));
    let mut flipped = first.pre_utxo[..1].to_vec();
    flipped[0].output.0[3] ^= 1;
    let mut differ = Tally::default();
    compare_utxos(&seeded.domain, id, &flipped, &mut differ).await;
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
