use std::collections::{BTreeMap, BTreeSet};

use crate::{
    cbor::TxIn,
    record::{CorpusKind, LedgerOutcome, PParams, Record, RecordId},
};

/// Why a run of records is left out of the judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcludeReason {
    Epoch,
    PParams,
    EpochAndPParams,
}

impl ExcludeReason {
    /// Whether the run spans more than one epoch.
    pub fn epoch(self) -> bool {
        matches!(self, ExcludeReason::Epoch | ExcludeReason::EpochAndPParams)
    }

    /// Whether the protocol parameters change inside the run.
    pub fn pparams(self) -> bool {
        matches!(
            self,
            ExcludeReason::PParams | ExcludeReason::EpochAndPParams
        )
    }
}

/// A run of records in one epoch under one parameter set.
#[derive(Debug, Clone)]
pub struct KeptRun {
    epoch: Option<u64>,
    pparams: PParams,
    records: Vec<Record>,
}

impl KeptRun {
    /// The epoch every record of the run is in.
    pub fn epoch(&self) -> Option<u64> {
        self.epoch
    }

    /// The protocol parameters every record of the run ran under.
    pub fn pparams(&self) -> &PParams {
        &self.pparams
    }

    /// The records in ledger order, at least one.
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// The records split into maximal sequences that share one slot.
    pub fn groups(&self) -> impl Iterator<Item = &[Record]> {
        self.records.chunk_by(|a, b| a.slot == b.slot)
    }

    /// The record whose ledger run made each output that a later record of the run can spend.
    pub fn makers(&self, kind: CorpusKind) -> BTreeMap<TxIn, RecordId> {
        let mut makers = BTreeMap::new();
        for record in &self.records {
            let LedgerOutcome::Accepted { post_utxo, .. } = &record.outcome else {
                continue;
            };
            let pre: BTreeSet<&TxIn> = record.pre_utxo.iter().map(|e| &e.input).collect();
            let id = RecordId {
                corpus: kind,
                n: record.n,
            };
            for entry in post_utxo.iter().filter(|e| !pre.contains(&e.input)) {
                makers.insert(entry.input, id);
            }
        }
        makers
    }
}

/// Where an input a record names comes from, as the ledger saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputOrigin {
    Seeded,
    InRun { made_by: RecordId },
    InGroup { made_by: RecordId },
    AbsentFromPre,
}

/// The origin of one input of a record, given the run's makers and the records of its slot group.
pub fn origin(
    input: &TxIn,
    record: &Record,
    makers: &BTreeMap<TxIn, RecordId>,
    group: &[Record],
) -> InputOrigin {
    if !record.pre_utxo.iter().any(|e| e.input == *input) {
        return InputOrigin::AbsentFromPre;
    }
    match makers.get(input) {
        None => InputOrigin::Seeded,
        Some(made_by) if group.iter().any(|r| r.n == made_by.n) => {
            InputOrigin::InGroup { made_by: *made_by }
        }
        Some(made_by) => InputOrigin::InRun { made_by: *made_by },
    }
}

/// A maximal sequence of records where each starts from the state the previous left.
#[derive(Debug, Clone)]
pub enum Run {
    Kept(KeptRun),
    Excluded {
        reason: ExcludeReason,
        first: u32,
        records: usize,
    },
}

fn classify(records: Vec<Record>) -> Run {
    let first = &records[0];
    let epoch_changes = records.iter().any(|r| r.epoch != first.epoch);
    let pparams_change = records.iter().any(|r| r.pparams != first.pparams);
    let reason = match (epoch_changes, pparams_change) {
        (false, false) => {
            return Run::Kept(KeptRun {
                epoch: first.epoch,
                pparams: first.pparams.clone(),
                records,
            })
        }
        (true, false) => ExcludeReason::Epoch,
        (false, true) => ExcludeReason::PParams,
        (true, true) => ExcludeReason::EpochAndPParams,
    };
    Run::Excluded {
        reason,
        first: first.n,
        records: records.len(),
    }
}

/// Splits records into runs at each record that does not start from the previous state.
pub fn split(records: Vec<Record>) -> Vec<Run> {
    let mut groups: Vec<Vec<Record>> = Vec::new();
    for record in records {
        match groups.last_mut() {
            Some(group) if record.follows_prev => group.push(record),
            _ => groups.push(vec![record]),
        }
    }
    groups.into_iter().map(classify).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cbor::tx_inputs, corpus};

    fn supp(n: u32) -> RecordId {
        RecordId {
            corpus: CorpusKind::Supp,
            n,
        }
    }

    /// The origin of every spend input of one supplement record.
    fn spend_origins(n: u32) -> Vec<InputOrigin> {
        let corpus = corpus::load(CorpusKind::Supp).unwrap();
        let run = corpus
            .kept()
            .find(|r| r.records().iter().any(|x| x.n == n))
            .unwrap();
        let makers = run.makers(CorpusKind::Supp);
        let group = run.groups().find(|g| g.iter().any(|x| x.n == n)).unwrap();
        let record = group.iter().find(|x| x.n == n).unwrap();
        let inputs = tx_inputs(&record.tx.0).unwrap();
        let spend = inputs
            .spend
            .iter()
            .chain(inputs.subs.iter().flat_map(|s| &s.spend));
        spend.map(|i| origin(i, record, &makers, group)).collect()
    }

    #[test]
    fn the_first_record_of_a_run_spends_only_seeded_inputs() {
        let origins = spend_origins(6);
        assert!(!origins.is_empty());
        assert!(
            origins.iter().all(|o| *o == InputOrigin::Seeded),
            "{origins:?}"
        );
    }

    #[test]
    fn an_output_made_earlier_in_the_slot_group_is_in_group() {
        assert!(spend_origins(2).contains(&InputOrigin::InGroup { made_by: supp(1) }));
    }

    #[test]
    fn an_output_made_in_an_earlier_slot_group_is_in_run() {
        assert!(spend_origins(9).contains(&InputOrigin::InRun { made_by: supp(8) }));
    }

    #[test]
    fn an_input_the_ledger_did_not_hold_is_absent_from_pre() {
        assert!(spend_origins(5).contains(&InputOrigin::AbsentFromPre));
    }
}
