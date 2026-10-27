use std::fmt;

use crate::record::{LedgerOutcome, Phase2, RecordId};

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

/// The judgement of one record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Agree,
    UnsupportedUnexpected { label: String },
    RefuseUnsupported { label: String },
    Disagree { ledger: String, dolos: String },
}

/// The outcome of one record from the ledger's answer and Dolos's answer.
pub fn outcome(ledger: &LedgerOutcome, phase2: Phase2, dolos: &DolosVerdict) -> Outcome {
    let disagree = || Outcome::Disagree {
        ledger: show_ledger(ledger, phase2),
        dolos: dolos.to_string(),
    };
    match (ledger, phase2, dolos) {
        (LedgerOutcome::Accepted { .. }, Phase2::Valid, DolosVerdict::Accepted) => Outcome::Agree,
        (LedgerOutcome::Accepted { .. }, Phase2::Invalid, DolosVerdict::Phase2Rejected) => {
            Outcome::Agree
        }
        (LedgerOutcome::Refused { .. }, _, DolosVerdict::Refused(_)) => Outcome::Agree,
        (LedgerOutcome::Accepted { .. }, _, DolosVerdict::Unsupported(label)) => {
            Outcome::UnsupportedUnexpected {
                label: label.clone(),
            }
        }
        (LedgerOutcome::Refused { .. }, _, DolosVerdict::Unsupported(label)) => {
            Outcome::RefuseUnsupported {
                label: label.clone(),
            }
        }
        (LedgerOutcome::Accepted { .. }, Phase2::Valid, DolosVerdict::Phase2Rejected)
        | (LedgerOutcome::Accepted { .. }, Phase2::Invalid, DolosVerdict::Accepted)
        | (LedgerOutcome::Accepted { .. }, _, DolosVerdict::Refused(_))
        | (LedgerOutcome::Refused { .. }, _, DolosVerdict::Accepted)
        | (LedgerOutcome::Refused { .. }, _, DolosVerdict::Phase2Rejected) => disagree(),
    }
}

/// The validation outcomes of one arm, with the records behind each one that is printed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ValidationTally {
    pub agree: usize,
    pub disagree: Vec<(RecordId, String)>,
    pub unsupported_unexpected: Vec<(RecordId, String)>,
    pub refuse_unsupported: Vec<(RecordId, String)>,
}

impl ValidationTally {
    /// Counts the outcome of one record.
    pub fn add(&mut self, id: RecordId, outcome: Outcome) {
        match outcome {
            Outcome::Agree => self.agree += 1,
            Outcome::UnsupportedUnexpected { label } => {
                self.unsupported_unexpected.push((id, format!("label={label}")))
            }
            Outcome::RefuseUnsupported { label } => {
                self.refuse_unsupported.push((id, format!("label={label}")))
            }
            Outcome::Disagree { ledger, dolos } => self
                .disagree
                .push((id, format!("ledger={ledger} dolos={dolos}"))),
        }
    }

    /// Whether the arm judged a record and no outcome fails it.
    pub fn pass(&self) -> bool {
        let ValidationTally {
            agree,
            disagree,
            unsupported_unexpected,
            refuse_unsupported,
        } = self;
        agree + disagree.len() + unsupported_unexpected.len() + refuse_unsupported.len() > 0
            && disagree.is_empty()
            && unsupported_unexpected.is_empty()
    }
}

impl fmt::Display for ValidationTally {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ValidationTally {
            agree,
            disagree,
            unsupported_unexpected,
            refuse_unsupported,
        } = self;
        write!(
            f,
            "VALIDATION agree={agree} disagree={} unsupported_unexpected={} refuse_unsupported={}",
            disagree.len(),
            unsupported_unexpected.len(),
            refuse_unsupported.len()
        )?;
        for (name, lines) in [
            ("DISAGREE", disagree),
            ("UNSUPPORTED_UNEXPECTED", unsupported_unexpected),
            ("REFUSE_UNSUPPORTED", refuse_unsupported),
        ] {
            for (id, text) in lines {
                write!(f, "\n{name} {id} {text}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::NonEmpty;

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

    #[test]
    fn matching_answers_agree() {
        assert_eq!(
            outcome(&accepted(), Phase2::Valid, &DolosVerdict::Accepted),
            Outcome::Agree
        );
        assert_eq!(
            outcome(&accepted(), Phase2::Invalid, &DolosVerdict::Phase2Rejected),
            Outcome::Agree
        );
        assert_eq!(
            outcome(
                &refused("BadInputsUTxO"),
                Phase2::Valid,
                &DolosVerdict::Refused("InputNotInUTxO".into())
            ),
            Outcome::Agree
        );
    }

    #[test]
    fn opposite_answers_disagree_and_name_both_sides() {
        let o = outcome(
            &refused("BadInputsUTxO"),
            Phase2::Valid,
            &DolosVerdict::Accepted,
        );
        let mut tally = ValidationTally::default();
        tally.add(
            RecordId {
                corpus: crate::record::CorpusKind::Supp,
                n: 5,
            },
            o,
        );
        assert!(!tally.pass());
        assert_eq!(
            tally.to_string(),
            "VALIDATION agree=0 disagree=1 unsupported_unexpected=0 refuse_unsupported=0\n\
             DISAGREE supp#5 ledger=refused[BadInputsUTxO] dolos=accepted"
        );
        assert!(matches!(
            outcome(&accepted(), Phase2::Valid, &DolosVerdict::Phase2Rejected),
            Outcome::Disagree { .. }
        ));
    }

    #[test]
    fn an_empty_tally_does_not_pass() {
        assert!(!ValidationTally::default().pass());
    }
}
