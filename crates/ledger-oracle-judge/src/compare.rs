use std::{collections::BTreeMap, fmt};

use crate::{
    manifest::{KnownList, Locus},
    record::{CorpusKind, RecordId},
};

/// How the ledger's bytes for one item stand against the node's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByteVerdict {
    Equal,
    Differ { at: usize },
    LedgerOnly(Vec<u8>),
    NodeOnly(Vec<u8>),
}

impl ByteVerdict {
    /// Compares the ledger's bytes with the node's, either side possibly absent.
    pub fn of(ledger: Option<&[u8]>, node: Option<&[u8]>) -> Option<Self> {
        match (ledger, node) {
            (None, None) => None,
            (Some(l), None) => Some(ByteVerdict::LedgerOnly(l.to_vec())),
            (None, Some(n)) => Some(ByteVerdict::NodeOnly(n.to_vec())),
            (Some(l), Some(n)) => Some(match first_difference(l, n) {
                None => ByteVerdict::Equal,
                Some(at) => ByteVerdict::Differ { at },
            }),
        }
    }

    /// Whether both sides hold the same bytes.
    pub fn agrees(&self) -> bool {
        matches!(self, ByteVerdict::Equal)
    }
}

impl fmt::Display for ByteVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ByteVerdict::Equal => write!(f, "equal"),
            ByteVerdict::Differ { at } => write!(f, "differ at byte {at}"),
            ByteVerdict::LedgerOnly(l) => write!(f, "ledger={} dolos=none", hex::encode(l)),
            ByteVerdict::NodeOnly(n) => write!(f, "ledger=none dolos={}", hex::encode(n)),
        }
    }
}

/// The index of the first byte where two byte strings differ.
pub fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(at) => Some(at),
        None if a.len() == b.len() => None,
        None => Some(a.len().min(b.len())),
    }
}

/// The count of items one comparison looked at and the ones that disagreed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tally {
    pub compared: usize,
    pub disagree: Vec<String>,
}

impl Tally {
    /// Counts one compared item, keeping its description when it disagrees.
    pub fn add(&mut self, agrees: bool, describe: impl FnOnce() -> String) {
        self.compared += 1;
        if !agrees {
            self.disagree.push(describe());
        }
    }
}

/// The state comparisons of one arm, one tally each.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StateTallies {
    pub utxo: Tally,
    pub as_output: Tally,
    pub in_run: usize,
    pub seeded_unjudged: usize,
    pub params: Tally,
    pub known: Vec<String>,
    pub known_stale: Vec<String>,
}

impl StateTallies {
    fn named(&self) -> [(&'static str, &Tally); 3] {
        [
            ("utxo", &self.utxo),
            ("as_output", &self.as_output),
            ("params", &self.params),
        ]
    }

    /// Whether every comparison looked at something and agreed on all of it.
    pub fn pass(&self) -> bool {
        let tallies = self.named();
        let agree = tallies
            .iter()
            .all(|(_, t)| t.compared > 0 && t.disagree.is_empty());
        agree && self.known_stale.is_empty()
    }

    /// Adds the served output comparisons of each record, passing a known record while it still disagrees.
    pub fn settle_as_output(
        &mut self,
        kind: CorpusKind,
        checks: BTreeMap<RecordId, Tally>,
        known: &KnownList,
    ) {
        for (id, tally) in &checks {
            self.as_output.compared += tally.compared;
            match known.get(&(Locus::AsOutput, *id)) {
                None => {
                    let lines = tally.disagree.iter().map(|d| format!("{id} {d}"));
                    self.as_output.disagree.extend(lines);
                }
                Some(row) if tally.disagree.is_empty() => {
                    self.known_stale
                        .push(format!("{id} row={row} locus=as_output"));
                }
                Some(row) => {
                    let lines = tally
                        .disagree
                        .iter()
                        .map(|d| format!("row={row} ids={id} as_output {d}"));
                    self.known.extend(lines);
                }
            }
        }
        for ((locus, id), row) in known {
            if *locus == Locus::AsOutput && id.corpus == kind && !checks.contains_key(id) {
                self.known_stale
                    .push(format!("{id} row={row} locus=as_output not_compared"));
            }
        }
    }
}

impl fmt::Display for StateTallies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let StateTallies {
            utxo,
            as_output,
            in_run,
            seeded_unjudged,
            params,
            known,
            known_stale,
        } = self;
        write!(
            f,
            "STATE compared utxo={} as_output={} of in_run={in_run} \
             seeded_unjudged={seeded_unjudged} params={}",
            utxo.compared, as_output.compared, params.compared
        )?;
        for line in known {
            write!(f, "\nKNOWN {line}")?;
        }
        for line in known_stale {
            write!(f, "\nKNOWN_STALE {line}")?;
        }
        for (name, t) in self.named() {
            if !t.disagree.is_empty() {
                write!(f, "\nSTATE {name} disagree={}", t.disagree.len())?;
                if name == "as_output" {
                    write!(f, " of in_run={in_run}")?;
                }
                for d in t.disagree.iter().take(20) {
                    write!(f, "\nDIFF {name} {d}")?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_difference_names_the_byte_or_the_shorter_length() {
        assert_eq!(first_difference(b"abc", b"abc"), None);
        assert_eq!(first_difference(b"abc", b"abd"), Some(2));
        assert_eq!(first_difference(b"ab", b"abc"), Some(2));
        assert_eq!(first_difference(b"", b""), None);
    }

    fn supp(n: u32) -> RecordId {
        RecordId {
            corpus: CorpusKind::Supp,
            n,
        }
    }

    fn checks(entries: &[(u32, &[&str])]) -> BTreeMap<RecordId, Tally> {
        let mut checks = BTreeMap::new();
        for (n, disagree) in entries {
            let tally: &mut Tally = checks.entry(supp(*n)).or_default();
            tally.add(true, String::new);
            for d in *disagree {
                tally.add(false, || d.to_string());
            }
        }
        checks
    }

    #[test]
    fn agreeing_records_off_the_known_list_print_nothing_and_pass() {
        let mut state = StateTallies::default();
        state.utxo.add(true, String::new);
        state.params.add(true, String::new);
        state.settle_as_output(
            CorpusKind::Supp,
            checks(&[(1, &[]), (2, &[])]),
            &KnownList::new(),
        );
        assert_eq!(state.as_output.compared, 2);
        assert!(state.known.is_empty() && state.known_stale.is_empty());
        assert!(state.pass(), "{state}");
    }

    #[test]
    fn a_known_record_passes_while_it_disagrees() {
        let mut state = StateTallies::default();
        state.utxo.add(true, String::new);
        state.params.add(true, String::new);
        let known = KnownList::from([((Locus::AsOutput, supp(9)), "S3".to_string())]);
        let diff = "ledger=ab dolos=none input=00#1 services=FetchBlock";
        state.settle_as_output(CorpusKind::Supp, checks(&[(9, &[diff])]), &known);
        assert_eq!(
            state.known,
            vec![format!("row=S3 ids=supp#9 as_output {diff}")]
        );
        assert!(state.as_output.disagree.is_empty());
        assert!(state.pass(), "{state}");
    }

    #[test]
    fn a_known_record_that_agrees_or_was_not_compared_is_stale() {
        let mut state = StateTallies::default();
        let known = KnownList::from([
            ((Locus::AsOutput, supp(9)), "S3".to_string()),
            ((Locus::AsOutput, supp(10)), "S3".to_string()),
            (
                (
                    Locus::AsOutput,
                    RecordId {
                        corpus: CorpusKind::Ic,
                        n: 9,
                    },
                ),
                "S3".to_string(),
            ),
            ((Locus::Validation, supp(11)), "V1".to_string()),
        ]);
        state.settle_as_output(CorpusKind::Supp, checks(&[(9, &[])]), &known);
        assert_eq!(
            state.known_stale,
            vec![
                "supp#9 row=S3 locus=as_output".to_string(),
                "supp#10 row=S3 locus=as_output not_compared".to_string()
            ]
        );
        assert!(!state.pass());
    }

    #[test]
    fn a_disagreement_off_the_known_list_fails_with_its_record() {
        let mut state = StateTallies::default();
        state.utxo.add(true, String::new);
        state.params.add(true, String::new);
        let diff = "ledger=ab dolos=none input=00#1 services=FetchBlock";
        state.settle_as_output(CorpusKind::Supp, checks(&[(9, &[diff])]), &KnownList::new());
        assert_eq!(state.as_output.disagree, vec![format!("supp#9 {diff}")]);
        assert!(!state.pass());
        assert!(state
            .to_string()
            .contains("\nSTATE as_output disagree=1 of in_run=0\n"));
    }

    #[test]
    fn byte_verdict_prints_each_side() {
        assert_eq!(
            ByteVerdict::of(Some(b"\x01"), Some(b"\x01")),
            Some(ByteVerdict::Equal)
        );
        assert_eq!(
            ByteVerdict::of(Some(b"\x01\x02"), Some(b"\x01\x03"))
                .unwrap()
                .to_string(),
            "differ at byte 1"
        );
        assert_eq!(
            ByteVerdict::of(Some(b"\xab"), None).unwrap().to_string(),
            "ledger=ab dolos=none"
        );
        assert_eq!(ByteVerdict::of(None, None), None);
    }
}
