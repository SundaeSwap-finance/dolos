use std::{collections::BTreeMap, fmt};

use crate::{
    manifest::Feature,
    record::{LedgerOutcome, Phase2, RecordId},
};

/// What Dolos answered when one transaction was submitted to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DolosVerdict {
    Accepted,
    Phase2Rejected,
    Unsupported(String),
    Refused(String),
}

impl fmt::Display for DolosVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DolosVerdict::Accepted => write!(f, "accepted"),
            DolosVerdict::Phase2Rejected => write!(f, "phase2_rejected"),
            DolosVerdict::Unsupported(label) => write!(f, "unsupported[{label}]"),
            DolosVerdict::Refused(kind) => write!(f, "refused[{kind}]"),
        }
    }
}

/// The ledger's answer for one record, printed for a disagreement.
pub fn show_ledger(outcome: &LedgerOutcome, phase2: Phase2) -> String {
    match (outcome, phase2) {
        (LedgerOutcome::Accepted { .. }, Phase2::Valid) => "accepted".to_string(),
        (LedgerOutcome::Accepted { .. }, Phase2::Invalid) => "accepted_phase2_invalid".to_string(),
        (LedgerOutcome::Refused { failure_names, .. }, _) => {
            let leaves: Vec<&str> = failure_names.iter().map(|p| p.last().as_str()).collect();
            format!("refused[{}]", leaves.join(","))
        }
    }
}

/// A judgement of one record that leaves the arm passing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pass {
    Agree,
    UnsupportedExpected { label: String },
    RefuseUnsupported { label: String },
    Known { row: String, shown: String },
}

/// A judgement of one record that fails the arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    Disagree { ledger: String, dolos: String },
    UnsupportedUnexpected { label: String },
    StaleManifest { declared: String, dolos: String },
    KnownStale { row: String },
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Disagree { ledger, dolos } => write!(f, "ledger={ledger} dolos={dolos}"),
            Fail::UnsupportedUnexpected { label } => write!(f, "label={label}"),
            Fail::StaleManifest { declared, dolos } => {
                write!(f, "declared={declared} dolos={dolos}")
            }
            Fail::KnownStale { row } => write!(f, "row={row}"),
        }
    }
}

/// The judgement of one record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass(Pass),
    Fail(Fail),
}

/// The outcome of one record from both answers and the feature it is declared unsupported for.
pub fn outcome(
    ledger: &LedgerOutcome,
    phase2: Phase2,
    dolos: &DolosVerdict,
    declared: Option<&Feature>,
) -> Outcome {
    use DolosVerdict as D;
    use LedgerOutcome as L;
    let disagree = || {
        Outcome::Fail(Fail::Disagree {
            ledger: show_ledger(ledger, phase2),
            dolos: dolos.to_string(),
        })
    };
    match (ledger, phase2, dolos, declared) {
        (L::Accepted { .. }, _, D::Unsupported(l), Some(f)) if *l == f.0 => {
            Outcome::Pass(Pass::UnsupportedExpected { label: l.clone() })
        }
        (L::Accepted { .. }, _, D::Unsupported(l), _) => {
            Outcome::Fail(Fail::UnsupportedUnexpected { label: l.clone() })
        }
        (L::Refused { .. }, _, D::Unsupported(l), _) => {
            Outcome::Pass(Pass::RefuseUnsupported { label: l.clone() })
        }
        (_, _, D::Accepted | D::Phase2Rejected | D::Refused(_), Some(f)) => {
            Outcome::Fail(Fail::StaleManifest {
                declared: f.to_string(),
                dolos: dolos.to_string(),
            })
        }
        (L::Accepted { .. }, Phase2::Valid, D::Accepted, None)
        | (L::Accepted { .. }, Phase2::Invalid, D::Phase2Rejected, None)
        | (L::Refused { .. }, _, D::Refused(_), None) => Outcome::Pass(Pass::Agree),
        (L::Accepted { .. }, Phase2::Valid, D::Phase2Rejected, None)
        | (L::Accepted { .. }, Phase2::Invalid, D::Accepted, None)
        | (L::Accepted { .. }, _, D::Refused(_), None)
        | (L::Refused { .. }, _, D::Accepted | D::Phase2Rejected, None) => disagree(),
    }
}

/// The outcome of a record on the known list, which passes only while the record still fails.
pub fn with_known(outcome: Outcome, row: Option<&String>) -> Outcome {
    match (row, outcome) {
        (None, outcome) => outcome,
        (Some(row), Outcome::Fail(fail)) => Outcome::Pass(Pass::Known {
            row: row.clone(),
            shown: fail.to_string(),
        }),
        (Some(row), Outcome::Pass(_)) => Outcome::Fail(Fail::KnownStale { row: row.clone() }),
    }
}

/// The validation outcomes of one arm, with the records behind each one that is printed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ValidationTally {
    pub agree: usize,
    pub unsupported_expected: BTreeMap<String, usize>,
    pub refuse_unsupported: Vec<(RecordId, String)>,
    pub known: Vec<(RecordId, String, String)>,
    pub disagree: Vec<(RecordId, String)>,
    pub unsupported_unexpected: Vec<(RecordId, String)>,
    pub stale_manifest: Vec<(RecordId, String)>,
    pub known_stale: Vec<(RecordId, String)>,
}

impl ValidationTally {
    /// Counts the outcome of one record.
    pub fn add(&mut self, id: RecordId, outcome: Outcome) {
        match outcome {
            Outcome::Pass(Pass::Agree) => self.agree += 1,
            Outcome::Pass(Pass::UnsupportedExpected { label }) => {
                *self.unsupported_expected.entry(label).or_default() += 1
            }
            Outcome::Pass(Pass::RefuseUnsupported { label }) => {
                self.refuse_unsupported.push((id, format!("label={label}")))
            }
            Outcome::Pass(Pass::Known { row, shown }) => self.known.push((id, row, shown)),
            Outcome::Fail(fail) => {
                let text = fail.to_string();
                match fail {
                    Fail::Disagree { .. } => self.disagree.push((id, text)),
                    Fail::UnsupportedUnexpected { .. } => {
                        self.unsupported_unexpected.push((id, text))
                    }
                    Fail::StaleManifest { .. } => self.stale_manifest.push((id, text)),
                    Fail::KnownStale { .. } => self.known_stale.push((id, text)),
                }
            }
        }
    }

    /// Whether the arm judged a record and no outcome fails it.
    pub fn pass(&self) -> bool {
        let ValidationTally {
            agree,
            unsupported_expected,
            refuse_unsupported,
            known,
            disagree,
            unsupported_unexpected,
            stale_manifest,
            known_stale,
        } = self;
        let passing = agree
            + unsupported_expected.values().sum::<usize>()
            + refuse_unsupported.len()
            + known.len();
        let failing = disagree.len()
            + unsupported_unexpected.len()
            + stale_manifest.len()
            + known_stale.len();
        passing + failing > 0 && failing == 0
    }
}

impl fmt::Display for ValidationTally {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ValidationTally {
            agree,
            unsupported_expected,
            refuse_unsupported,
            known,
            disagree,
            unsupported_unexpected,
            stale_manifest,
            known_stale,
        } = self;
        write!(
            f,
            "VALIDATION agree={agree} disagree={} unsupported_expected={} \
             unsupported_unexpected={} refuse_unsupported={} stale_manifest={} known={} \
             known_stale={}",
            disagree.len(),
            unsupported_expected.values().sum::<usize>(),
            unsupported_unexpected.len(),
            refuse_unsupported.len(),
            stale_manifest.len(),
            known.len(),
            known_stale.len()
        )?;
        for (id, row, shown) in known {
            write!(f, "\nKNOWN row={row} ids={id} {shown}")?;
        }
        for (name, lines) in [
            ("DISAGREE", disagree),
            ("UNSUPPORTED_UNEXPECTED", unsupported_unexpected),
            ("STALE_MANIFEST", stale_manifest),
            ("KNOWN_STALE", known_stale),
            ("REFUSE_UNSUPPORTED", refuse_unsupported),
        ] {
            for (id, text) in lines {
                write!(f, "\n{name} {id} {text}")?;
            }
        }
        for (label, count) in unsupported_expected {
            write!(f, "\nUNSUPPORTED_EXPECTED label={label} count={count}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{CorpusKind, NonEmpty};

    fn accepted() -> LedgerOutcome {
        LedgerOutcome::Accepted {
            post_utxo: vec![],
            accounts_post: serde_json::Value::Null,
        }
    }

    fn refused(leaf: &str) -> LedgerOutcome {
        LedgerOutcome::Refused {
            failures: String::new(),
            failure_names: NonEmpty {
                head: NonEmpty {
                    head: "UtxoFailure".to_string(),
                    tail: vec![leaf.to_string()],
                },
                tail: vec![],
            },
        }
    }

    fn supp(n: u32) -> RecordId {
        RecordId {
            corpus: CorpusKind::Supp,
            n,
        }
    }

    fn feature(label: &str) -> Feature {
        Feature(label.to_string())
    }

    #[test]
    fn matching_answers_agree() {
        let agree = Outcome::Pass(Pass::Agree);
        assert_eq!(
            outcome(&accepted(), Phase2::Valid, &DolosVerdict::Accepted, None),
            agree
        );
        assert_eq!(
            outcome(
                &accepted(),
                Phase2::Invalid,
                &DolosVerdict::Phase2Rejected,
                None
            ),
            agree
        );
        assert_eq!(
            outcome(
                &refused("BadInputsUTxO"),
                Phase2::Valid,
                &DolosVerdict::Refused("InputNotInUTxO".into()),
                None
            ),
            agree
        );
    }

    #[test]
    fn opposite_answers_disagree_and_name_both_sides() {
        let o = outcome(
            &refused("BadInputsUTxO"),
            Phase2::Valid,
            &DolosVerdict::Accepted,
            None,
        );
        let mut tally = ValidationTally::default();
        tally.add(supp(5), o);
        assert!(!tally.pass());
        assert_eq!(
            tally.to_string(),
            "VALIDATION agree=0 disagree=1 unsupported_expected=0 unsupported_unexpected=0 \
             refuse_unsupported=0 stale_manifest=0 known=0 known_stale=0\n\
             DISAGREE supp#5 ledger=refused[BadInputsUTxO] dolos=accepted"
        );
    }

    #[test]
    fn a_declared_label_passes_only_when_dolos_names_it() {
        let unsupported = DolosVerdict::Unsupported("certificate kind".into());
        assert_eq!(
            outcome(
                &accepted(),
                Phase2::Valid,
                &unsupported,
                Some(&feature("certificate kind"))
            ),
            Outcome::Pass(Pass::UnsupportedExpected {
                label: "certificate kind".into()
            })
        );
        assert_eq!(
            outcome(
                &accepted(),
                Phase2::Valid,
                &unsupported,
                Some(&feature("pointer address"))
            ),
            Outcome::Fail(Fail::UnsupportedUnexpected {
                label: "certificate kind".into()
            })
        );
        assert_eq!(
            outcome(&accepted(), Phase2::Valid, &unsupported, None),
            Outcome::Fail(Fail::UnsupportedUnexpected {
                label: "certificate kind".into()
            })
        );
    }

    #[test]
    fn a_declared_record_that_dolos_supports_is_a_stale_manifest() {
        assert_eq!(
            outcome(
                &accepted(),
                Phase2::Valid,
                &DolosVerdict::Accepted,
                Some(&feature("certificate kind"))
            ),
            Outcome::Fail(Fail::StaleManifest {
                declared: "certificate kind".into(),
                dolos: "accepted".into()
            })
        );
    }

    #[test]
    fn a_known_record_passes_while_it_fails_and_fails_once_it_agrees() {
        let row = "V2".to_string();
        let failing = outcome(
            &accepted(),
            Phase2::Valid,
            &DolosVerdict::Refused("InputNotInUTxO".into()),
            None,
        );
        assert_eq!(
            with_known(failing.clone(), None),
            failing,
            "a record off the list keeps its outcome"
        );
        assert_eq!(
            with_known(failing, Some(&row)),
            Outcome::Pass(Pass::Known {
                row: row.clone(),
                shown: "ledger=accepted dolos=refused[InputNotInUTxO]".into()
            })
        );
        assert_eq!(
            with_known(Outcome::Pass(Pass::Agree), Some(&row)),
            Outcome::Fail(Fail::KnownStale { row })
        );
    }

    #[test]
    fn an_empty_tally_does_not_pass() {
        assert!(!ValidationTally::default().pass());
    }
}
