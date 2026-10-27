use crate::record::{PParams, Record};

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
        matches!(self, ExcludeReason::PParams | ExcludeReason::EpochAndPParams)
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
