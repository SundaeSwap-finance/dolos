use std::{collections::BTreeMap, fmt};

use crate::{
    manifest::{KnownList, Locus},
    record::{CorpusKind, RecordId},
};

/// The result of comparing the ledger's bytes for one item with the node's.
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
            (Some(l), node) => Some(ByteVerdict::against(l, node)),
            (None, Some(n)) => Some(ByteVerdict::NodeOnly(n.to_vec())),
        }
    }

    /// Compares the ledger's bytes with the node's, the node's possibly absent.
    pub fn against(ledger: &[u8], node: Option<&[u8]>) -> Self {
        match node {
            None => ByteVerdict::LedgerOnly(ledger.to_vec()),
            Some(n) => match first_difference(ledger, n) {
                None => ByteVerdict::Equal,
                Some(at) => ByteVerdict::Differ { at },
            },
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

    /// Counts the items and keeps the disagreements of another tally.
    pub fn merge(&mut self, other: Tally) {
        self.compared += other.compared;
        self.disagree.extend(other.disagree);
    }
}

/// A slot group whose block the node did not apply, with why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollFail {
    pub first: RecordId,
    pub last: RecordId,
    pub records: usize,
    pub slot: u64,
    pub cause: String,
}

impl fmt::Display for RollFail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let RollFail {
            first,
            last,
            records,
            slot,
            cause,
        } = self;
        write!(
            f,
            "group={first}..{last} records={records} slot={slot} {cause}"
        )
    }
}

/// The state comparisons of one arm, one tally each.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StateTallies {
    pub roll_fail: Vec<RollFail>,
    pub utxo: Tally,
    pub as_output: Tally,
    pub in_run: usize,
    pub seeded_unjudged: usize,
    pub accounts: Tally,
    pub undo: Tally,
    pub params: Tally,
    pub readtx: Tally,
    pub script: Tally,
    pub datum: Tally,
    pub known: Vec<String>,
    pub known_fired: KnownList,
    pub known_stale: Vec<String>,
}

impl StateTallies {
    fn named(&self) -> [(&'static str, &Tally); 8] {
        [
            ("utxo", &self.utxo),
            ("as_output", &self.as_output),
            ("accounts", &self.accounts),
            ("undo", &self.undo),
            ("params", &self.params),
            ("readtx", &self.readtx),
            ("script", &self.script),
            ("datum", &self.datum),
        ]
    }

    /// Whether every block rolled and every comparison looked at something and agreed on all of it.
    pub fn pass(&self) -> bool {
        let tallies = self.named();
        let agree = tallies
            .iter()
            .all(|(_, t)| t.compared > 0 && t.disagree.is_empty());
        agree && self.known_stale.is_empty() && self.roll_fail.is_empty()
    }

    /// The count of records in slot groups whose block the node did not apply.
    pub fn rolled_failed(&self) -> usize {
        self.roll_fail.iter().map(|r| r.records).sum()
    }

    /// Adds the served output comparisons of each record, passing a known record while it still disagrees.
    pub fn settle_as_output(
        &mut self,
        kind: CorpusKind,
        checks: BTreeMap<RecordId, Tally>,
        known: &KnownList,
    ) {
        self.settle(Locus::AsOutput, |s| &mut s.as_output, kind, checks, known);
    }

    /// Adds the account comparisons of each record, passing a known record while it still disagrees.
    pub fn settle_accounts(
        &mut self,
        kind: CorpusKind,
        checks: BTreeMap<RecordId, Tally>,
        known: &KnownList,
    ) {
        self.settle(Locus::Accounts, |s| &mut s.accounts, kind, checks, known);
    }

    /// Adds the comparisons of each record to the tally of `locus`, passing a known record while it still disagrees.
    fn settle(
        &mut self,
        locus: Locus,
        tally_of: fn(&mut StateTallies) -> &mut Tally,
        kind: CorpusKind,
        checks: BTreeMap<RecordId, Tally>,
        known: &KnownList,
    ) {
        for (id, tally) in &checks {
            tally_of(self).compared += tally.compared;
            match known.get(&(locus, *id)) {
                None => {
                    let lines = tally.disagree.iter().map(|d| format!("{id} {d}"));
                    tally_of(self).disagree.extend(lines);
                }
                Some(row) if tally.disagree.is_empty() => {
                    self.known_stale
                        .push(format!("{id} row={row} locus={locus}"));
                }
                Some(row) => {
                    let lines = tally
                        .disagree
                        .iter()
                        .map(|d| format!("row={row} ids={id} {locus} {d}"));
                    self.known.extend(lines);
                    self.known_fired.insert((locus, *id), *row);
                }
            }
        }
        for ((entry, id), row) in known {
            if *entry == locus && id.corpus == kind && !checks.contains_key(id) {
                self.known_stale
                    .push(format!("{id} row={row} locus={locus} not_compared"));
            }
        }
    }
}

impl fmt::Display for StateTallies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let StateTallies {
            roll_fail,
            utxo,
            as_output,
            in_run,
            seeded_unjudged,
            accounts,
            undo,
            params,
            readtx,
            script,
            datum,
            known,
            known_fired: _,
            known_stale,
        } = self;
        write!(
            f,
            "STATE compared utxo={} as_output={} of in_run={in_run} \
             seeded_unjudged={seeded_unjudged} accounts={} undo={} params={} readtx={} script={} \
             datum={} rolled_failed={}",
            utxo.compared,
            as_output.compared,
            accounts.compared,
            undo.compared,
            params.compared,
            readtx.compared,
            script.compared,
            datum.compared,
            self.rolled_failed()
        )?;
        for line in roll_fail {
            write!(f, "\nROLL_FAIL {line}")?;
        }
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
    use crate::row::Row;

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

    fn compared() -> StateTallies {
        let mut state = StateTallies::default();
        state.utxo.add(true, String::new);
        state.accounts.add(true, String::new);
        state.undo.add(true, String::new);
        state.params.add(true, String::new);
        state.readtx.add(true, String::new);
        state.script.add(true, String::new);
        state.datum.add(true, String::new);
        state
    }

    #[test]
    fn a_comparison_that_looked_at_nothing_fails() {
        let mut state = compared();
        state.as_output.add(true, String::new);
        assert!(state.pass(), "{state}");
        let empties: [fn(&mut StateTallies) -> &mut Tally; 8] = [
            |s| &mut s.utxo,
            |s| &mut s.as_output,
            |s| &mut s.accounts,
            |s| &mut s.undo,
            |s| &mut s.params,
            |s| &mut s.readtx,
            |s| &mut s.script,
            |s| &mut s.datum,
        ];
        for empty in empties {
            let mut state = state.clone();
            *empty(&mut state) = Tally::default();
            assert!(!state.pass(), "{state}");
        }
        assert!(state
            .to_string()
            .contains(" seeded_unjudged=0 accounts=1 undo=1 params=1 readtx=1 script=1 datum=1"));
    }

    #[test]
    fn a_group_that_did_not_roll_prints_one_line_and_fails() {
        let mut state = compared();
        state.as_output.add(true, String::new);
        assert!(state.pass(), "{state}");
        assert!(state.to_string().contains(" datum=1 rolled_failed=0"));
        assert!(!state.to_string().contains("ROLL_FAIL"));
        state.roll_fail.push(RollFail {
            first: supp(3),
            last: supp(5),
            records: 3,
            slot: 40,
            cause: "panic at=a.rs:1:2 explicit panic".to_string(),
        });
        assert!(!state.pass(), "{state}");
        let text = state.to_string();
        assert!(text.contains(" datum=1 rolled_failed=3\n"), "{text}");
        assert!(
            text.contains(
                "\nROLL_FAIL group=supp#3..supp#5 records=3 slot=40 panic at=a.rs:1:2 explicit panic"
            ),
            "{text}"
        );
    }

    #[test]
    fn agreeing_records_off_the_known_list_print_nothing_and_pass() {
        let mut state = compared();
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
        let mut state = compared();
        let known = KnownList::from([((Locus::AsOutput, supp(9)), Row::S3)]);
        let diff = "ledger=ab dolos=none input=00#1 services=FetchBlock";
        state.settle_as_output(CorpusKind::Supp, checks(&[(9, &[diff])]), &known);
        assert_eq!(
            state.known,
            vec![format!("row=S3 ids=supp#9 as_output {diff}")]
        );
        assert_eq!(state.known_fired, known);
        assert!(state.as_output.disagree.is_empty());
        assert!(state.pass(), "{state}");
    }

    #[test]
    fn a_known_record_that_agrees_or_was_not_compared_is_stale() {
        let mut state = StateTallies::default();
        let known = KnownList::from([
            ((Locus::AsOutput, supp(9)), Row::S3),
            ((Locus::AsOutput, supp(10)), Row::S3),
            (
                (
                    Locus::AsOutput,
                    RecordId {
                        corpus: CorpusKind::Ic,
                        n: 9,
                    },
                ),
                Row::S3,
            ),
            ((Locus::Validation, supp(11)), Row::V1),
        ]);
        state.settle_as_output(CorpusKind::Supp, checks(&[(9, &[])]), &known);
        assert_eq!(
            state.known_stale,
            vec![
                "supp#9 row=S3 locus=as_output".to_string(),
                "supp#10 row=S3 locus=as_output not_compared".to_string()
            ]
        );
        assert!(state.known_fired.is_empty());
        assert!(!state.pass());
    }

    #[test]
    fn an_account_disagreement_off_the_known_list_fails_and_a_known_one_passes() {
        let diff = "account=keyHash-00 ledger=balance=0,pool=none,drep=none dolos=balance=0";
        let mut state = compared();
        state.as_output.add(true, String::new);
        let as_output = KnownList::from([((Locus::AsOutput, supp(9)), Row::S3)]);
        state.settle_accounts(CorpusKind::Supp, checks(&[(9, &[diff])]), &as_output);
        assert_eq!(state.accounts.disagree, vec![format!("supp#9 {diff}")]);
        assert!(state.known.is_empty());
        assert!(!state.pass(), "{state}");

        let mut state = compared();
        state.as_output.add(true, String::new);
        let known = KnownList::from([((Locus::Accounts, supp(9)), Row::S18)]);
        state.settle_accounts(CorpusKind::Supp, checks(&[(9, &[diff])]), &known);
        assert_eq!(
            state.known,
            vec![format!("row=S18 ids=supp#9 accounts {diff}")]
        );
        assert_eq!(state.known_fired, known);
        assert!(state.accounts.disagree.is_empty());
        assert!(state.pass(), "{state}");
    }

    #[test]
    fn a_disagreement_off_the_known_list_fails_with_its_record() {
        let mut state = compared();
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
