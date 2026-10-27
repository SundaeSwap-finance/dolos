use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use crate::{
    manifest::Feature,
    record::{FailurePath, LedgerOutcome, NonEmpty, Phase2, RecordId},
    refusal::RefusalClass,
    row::Row,
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
            let leaves: Vec<&str> = failure_names.iter().map(FailurePath::leaf).collect();
            format!("refused[{}]", leaves.join(","))
        }
    }
}

/// The outcome of a refusal by both sides, which agrees when the node's class is one of the ledger's.
fn refusal_outcome(failure_names: &NonEmpty<FailurePath>, ledger: String, kind: &str) -> Outcome {
    let ledger_classes: BTreeSet<RefusalClass> = failure_names.iter().map(|p| p.class).collect();
    match RefusalClass::of_dolos_kind(kind) {
        Some(class) if ledger_classes.contains(&class) => Outcome::Pass(Pass::Agree),
        dolos_class => {
            let ledger_classes: Vec<&str> = ledger_classes.iter().map(|c| c.name()).collect();
            Outcome::Fail(Fail::ReasonMismatch {
                ledger,
                ledger_classes: ledger_classes.join(","),
                dolos: format!("refused[{kind}]"),
                dolos_class: dolos_class.map_or("none", RefusalClass::name).to_string(),
            })
        }
    }
}

/// A judgement of one record that leaves the arm passing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pass {
    Agree,
    UnsupportedExpected { label: String },
    RefuseUnsupported { label: String },
    Known { row: Row, shown: String },
}

/// A judgement of one record that fails the arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    Disagree {
        ledger: String,
        dolos: String,
    },
    ReasonMismatch {
        ledger: String,
        ledger_classes: String,
        dolos: String,
        dolos_class: String,
    },
    UnsupportedUnexpected {
        label: String,
    },
    StaleManifest {
        declared: String,
        dolos: String,
    },
    KnownStale {
        row: Row,
    },
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Disagree { ledger, dolos } => write!(f, "ledger={ledger} dolos={dolos}"),
            Fail::ReasonMismatch {
                ledger,
                ledger_classes,
                dolos,
                dolos_class,
            } => write!(
                f,
                "ledger={ledger} ledger_class={ledger_classes} dolos={dolos} \
                 dolos_class={dolos_class}"
            ),
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
        | (L::Accepted { .. }, Phase2::Invalid, D::Phase2Rejected, None) => {
            Outcome::Pass(Pass::Agree)
        }
        (L::Refused { failure_names, .. }, _, D::Refused(kind), None) => {
            refusal_outcome(failure_names, show_ledger(ledger, phase2), kind)
        }
        (L::Accepted { .. }, Phase2::Valid, D::Phase2Rejected, None)
        | (L::Accepted { .. }, Phase2::Invalid, D::Accepted, None)
        | (L::Accepted { .. }, _, D::Refused(_), None)
        | (L::Refused { .. }, _, D::Accepted | D::Phase2Rejected, None) => disagree(),
    }
}

/// The feature a record would be declared unsupported for, from a ledger that accepted it and a Dolos that named it unsupported.
pub fn declares(ledger: &LedgerOutcome, dolos: &DolosVerdict) -> Option<Feature> {
    match (ledger, dolos) {
        (LedgerOutcome::Accepted { .. }, DolosVerdict::Unsupported(label)) => {
            Some(Feature(label.clone()))
        }
        _ => None,
    }
}

/// The outcome of a record on the known list, which passes only while the record still fails.
pub fn with_known(outcome: Outcome, row: Option<&Row>) -> Outcome {
    match (row, outcome) {
        (None, outcome) => outcome,
        (Some(&row), Outcome::Fail(fail)) => Outcome::Pass(Pass::Known {
            row,
            shown: fail.to_string(),
        }),
        (Some(&row), Outcome::Pass(_)) => Outcome::Fail(Fail::KnownStale { row }),
    }
}

/// A record whose outputs are seeded for the records that spend them, made only by `seeds_dependents`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SeedingMaker(RecordId);

impl Borrow<RecordId> for SeedingMaker {
    fn borrow(&self) -> &RecordId {
        &self.0
    }
}

/// The record as a seeding maker when Dolos did not take it and its outcome passes.
pub fn seeds_dependents(
    id: RecordId,
    dolos: &DolosVerdict,
    outcome: &Outcome,
) -> Option<SeedingMaker> {
    use DolosVerdict as D;
    match (dolos, outcome) {
        (D::Accepted, Outcome::Pass(_) | Outcome::Fail(_)) => None,
        (D::Phase2Rejected | D::Unsupported(_) | D::Refused(_), Outcome::Fail(_)) => None,
        (D::Phase2Rejected | D::Unsupported(_) | D::Refused(_), Outcome::Pass(_)) => {
            Some(SeedingMaker(id))
        }
    }
}

/// The validation outcomes of one arm, with the records behind each one that is printed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ValidationTally {
    pub dependent: Vec<(RecordId, BTreeSet<RecordId>)>,
    pub agree: usize,
    pub unsupported_expected: BTreeMap<String, usize>,
    pub refuse_unsupported: Vec<(RecordId, String)>,
    pub known: Vec<(RecordId, Row, String)>,
    pub disagree: Vec<(RecordId, String)>,
    pub reason_mismatch: Vec<(RecordId, String)>,
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
                    Fail::ReasonMismatch { .. } => self.reason_mismatch.push((id, text)),
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
            dependent: _,
            agree,
            unsupported_expected,
            refuse_unsupported,
            known,
            disagree,
            reason_mismatch,
            unsupported_unexpected,
            stale_manifest,
            known_stale,
        } = self;
        let passing = agree
            + unsupported_expected.values().sum::<usize>()
            + refuse_unsupported.len()
            + known.len();
        let failing = disagree.len()
            + reason_mismatch.len()
            + unsupported_unexpected.len()
            + stale_manifest.len()
            + known_stale.len();
        passing + failing > 0 && failing == 0
    }
}

impl fmt::Display for ValidationTally {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ValidationTally {
            dependent,
            agree,
            unsupported_expected,
            refuse_unsupported,
            known,
            disagree,
            reason_mismatch,
            unsupported_unexpected,
            stale_manifest,
            known_stale,
        } = self;
        write!(
            f,
            "VALIDATION agree={agree} disagree={} reason_mismatch={} unsupported_expected={} \
             unsupported_unexpected={} refuse_unsupported={} stale_manifest={} dependent={} \
             known={} known_stale={}",
            disagree.len(),
            reason_mismatch.len(),
            unsupported_expected.values().sum::<usize>(),
            unsupported_unexpected.len(),
            refuse_unsupported.len(),
            stale_manifest.len(),
            dependent.len(),
            known.len(),
            known_stale.len()
        )?;
        let dependent: Vec<String> = dependent
            .iter()
            .map(|(id, makers)| {
                let makers: Vec<String> = makers.iter().map(RecordId::to_string).collect();
                format!("{id}({})", makers.join(","))
            })
            .collect();
        write!(f, "\nDEPENDENT ids={}", dependent.join(","))?;
        for (id, row, shown) in known {
            write!(f, "\nKNOWN row={row} ids={id} {shown}")?;
        }
        for (name, lines) in [
            ("DISAGREE", disagree),
            ("REASON_MISMATCH", reason_mismatch),
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
    use crate::record::CorpusKind;

    fn accepted() -> LedgerOutcome {
        LedgerOutcome::Accepted {
            post_utxo: vec![],
            accounts_post: Default::default(),
        }
    }

    fn path(leaf: &str) -> FailurePath {
        FailurePath {
            names: NonEmpty {
                head: "UtxoFailure".to_string(),
                tail: vec![leaf.to_string()],
            },
            class: RefusalClass::of_ledger_leaf(leaf).unwrap(),
        }
    }

    fn refused_by(leaves: &[&str]) -> LedgerOutcome {
        LedgerOutcome::Refused {
            failures: String::new(),
            failure_names: NonEmpty {
                head: path(leaves[0]),
                tail: leaves[1..].iter().map(|l| path(l)).collect(),
            },
        }
    }

    fn refused(leaf: &str) -> LedgerOutcome {
        refused_by(&[leaf])
    }

    #[test]
    fn a_refusal_in_any_of_the_ledger_classes_agrees() {
        let ledger = refused_by(&["SubMissingTxMetadata", "BadInputsUTxO"]);
        for kind in ["MetadataHash", "InputNotInUTxO"] {
            assert_eq!(
                outcome(
                    &ledger,
                    Phase2::Valid,
                    &DolosVerdict::Refused(kind.into()),
                    None
                ),
                Outcome::Pass(Pass::Agree),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_refusal_outside_the_ledger_classes_is_a_reason_mismatch() {
        let ledger = refused_by(&["BadInputsUTxO", "BadInputsUTxO"]);
        let mut tally = ValidationTally::default();
        for (n, kind) in [(5, "PreservationOfValue"), (6, "DijkstraMissingParameters")] {
            let dolos = DolosVerdict::Refused(kind.into());
            tally.add(supp(n), outcome(&ledger, Phase2::Valid, &dolos, None));
        }
        assert!(!tally.pass());
        let text = tally.to_string();
        assert!(
            text.contains(
                "\nREASON_MISMATCH supp#5 ledger=refused[BadInputsUTxO,BadInputsUTxO] \
                 ledger_class=inputs dolos=refused[PreservationOfValue] dolos_class=value"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "\nREASON_MISMATCH supp#6 ledger=refused[BadInputsUTxO,BadInputsUTxO] \
                 ledger_class=inputs dolos=refused[DijkstraMissingParameters] dolos_class=none"
            ),
            "{text}"
        );
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
            "VALIDATION agree=0 disagree=1 reason_mismatch=0 unsupported_expected=0 \
             unsupported_unexpected=0 \
             refuse_unsupported=0 stale_manifest=0 dependent=0 known=0 known_stale=0\n\
             DEPENDENT ids=\n\
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
        let row = Row::V2;
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
                row,
                shown: "ledger=accepted dolos=refused[InputNotInUTxO]".into()
            })
        );
        assert_eq!(
            with_known(Outcome::Pass(Pass::Agree), Some(&row)),
            Outcome::Fail(Fail::KnownStale { row })
        );
    }

    #[test]
    fn only_a_passing_maker_dolos_did_not_take_seeds_its_dependents() {
        let unsupported = DolosVerdict::Unsupported("certificate kind".into());
        let expected = outcome(
            &accepted(),
            Phase2::Valid,
            &unsupported,
            Some(&feature("certificate kind")),
        );
        let unexpected = outcome(&accepted(), Phase2::Valid, &unsupported, None);
        let held = Outcome::Pass(Pass::Agree);
        assert_eq!(
            seeds_dependents(supp(3), &unsupported, &expected),
            Some(SeedingMaker(supp(3)))
        );
        assert_eq!(seeds_dependents(supp(3), &unsupported, &unexpected), None);
        assert_eq!(
            seeds_dependents(supp(3), &DolosVerdict::Accepted, &held),
            None
        );
    }

    #[test]
    fn only_an_accepted_record_dolos_names_unsupported_is_declared() {
        let unsupported = DolosVerdict::Unsupported("non-vkey witnesses".into());
        assert_eq!(declares(&refused("BadInputsUTxO"), &unsupported), None);
        assert_eq!(declares(&accepted(), &DolosVerdict::Accepted), None);
        let refused_kind = DolosVerdict::Refused("InputNotInUTxO".into());
        assert_eq!(declares(&accepted(), &refused_kind), None);
        assert_eq!(
            declares(&accepted(), &unsupported),
            Some(feature("non-vkey witnesses"))
        );
    }

    #[test]
    fn an_empty_tally_does_not_pass() {
        assert!(!ValidationTally::default().pass());
    }
}
