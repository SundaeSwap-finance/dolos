use std::{collections::BTreeMap, fmt, ops::Range};

use serde::Deserialize;

use crate::{
    compare::Tally,
    manifest::{KnownLive, LiveLocus},
    row::{judged_by, Row},
};

/// The node's tip as its tip query prints it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeTip {
    pub slot: u64,
    pub epoch: u64,
    pub era: String,
    pub slot_in_epoch: u64,
    pub slots_to_epoch_end: u64,
}

impl NodeTip {
    /// The slots of the tip's epoch, none when the tip's counts do not fit its slot.
    pub fn epoch_slots(&self) -> Option<Range<u64>> {
        let start = self.slot.checked_sub(self.slot_in_epoch)?;
        let end = self.slot.checked_add(self.slots_to_epoch_end)?;
        Some(start..end)
    }
}

/// Reads taken while the node stayed in the epoch that holds the served ledger slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aligned<T> {
    epoch: u64,
    value: T,
}

impl<T> Aligned<T> {
    /// Pairs the reads with the epoch that contains both node tips and the served slot, none otherwise.
    pub fn new(before: &NodeTip, after: &NodeTip, served_slot: u64, value: T) -> Option<Self> {
        let slots = before.epoch_slots()?;
        let aligned = before.epoch == after.epoch && slots.contains(&served_slot);
        aligned.then_some(Aligned {
            epoch: before.epoch,
            value,
        })
    }

    /// The epoch that contains the reads.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The aligned reads.
    pub fn value(&self) -> &T {
        &self.value
    }
}

/// The result of comparing the last era name Dolos serves with the node's era.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EraName {
    Agree(String),
    Disagree {
        served: Option<String>,
        node: String,
    },
}

impl EraName {
    /// Whether the served name is the node's era.
    pub fn agrees(&self) -> bool {
        matches!(self, EraName::Agree(_))
    }
}

impl fmt::Display for EraName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EraName::Agree(name) => write!(f, "name={name}"),
            EraName::Disagree { served, node } => {
                let served = served.as_deref().unwrap_or("none");
                write!(f, "name={served} node={node}")
            }
        }
    }
}

/// Compares the name of the last served era with the node's era, ignoring case.
pub fn era_name(served: &[String], node: &str) -> EraName {
    match served.last() {
        Some(name) if name.eq_ignore_ascii_case(node) => EraName::Agree(name.clone()),
        last => EraName::Disagree {
            served: last.cloned(),
            node: node.to_string(),
        },
    }
}

/// The lines and verdict of the live comparisons, judged against the known list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    pub lines: Vec<String>,
    pub known_fired: KnownLive,
    pub pass: bool,
}

/// Judges each live locus's tally, failing one that compared nothing, disagrees off the list, or agrees on it.
pub fn settle(tallies: &BTreeMap<LiveLocus, Tally>, known: &KnownLive) -> Settled {
    let mut settled = Settled {
        lines: Vec::new(),
        known_fired: KnownLive::new(),
        pass: true,
    };
    for &locus in LiveLocus::ALL {
        let none = Tally::default();
        let tally = tallies.get(&locus).unwrap_or(&none);
        let mut fail = |line: String| {
            settled.lines.push(line);
            settled.pass = false;
        };
        if tally.compared == 0 {
            fail(format!("LIVE-GAP locus={locus} compared=0"));
            continue;
        }
        match known.get(&locus) {
            Some(row) if tally.disagree.is_empty() => {
                fail(format!("KNOWN_STALE row={row} locus={locus}"));
            }
            Some(row) => {
                let lines = tally.disagree.iter();
                let lines = lines.map(|d| format!("KNOWN row={row} locus={locus} {d}"));
                settled.lines.extend(lines);
                settled.known_fired.insert(locus, *row);
            }
            None => {
                let rows: Vec<String> = judged_by(locus.check())
                    .iter()
                    .map(Row::to_string)
                    .collect();
                let rows = rows.join(",");
                for d in &tally.disagree {
                    fail(format!("LIVE-DIFF row={rows} locus={locus} {d}"));
                }
            }
        }
    }
    settled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tip() -> NodeTip {
        NodeTip {
            slot: 829955,
            epoch: 38,
            era: "Dijkstra".into(),
            slot_in_epoch: 9155,
            slots_to_epoch_end: 12445,
        }
    }

    #[test]
    fn reads_inside_one_epoch_are_aligned() {
        let later = NodeTip {
            slot: 829990,
            slot_in_epoch: 9190,
            slots_to_epoch_end: 12410,
            ..tip()
        };
        let aligned = Aligned::new(&tip(), &later, 820800, ()).unwrap();
        assert_eq!(aligned.epoch(), 38);
        assert!(Aligned::new(&tip(), &later, 842399, ()).is_some());
    }

    #[test]
    fn reads_across_an_epoch_or_outside_it_are_not_aligned() {
        let next = NodeTip {
            slot: 842400,
            epoch: 39,
            slot_in_epoch: 0,
            slots_to_epoch_end: 21600,
            ..tip()
        };
        assert_eq!(Aligned::new(&tip(), &next, 829955, ()), None);
        assert_eq!(Aligned::new(&tip(), &tip(), 842400, ()), None);
        assert_eq!(Aligned::new(&tip(), &tip(), 820799, ()), None);
    }

    fn tally(compared: usize, disagree: &[&str]) -> Tally {
        Tally {
            compared,
            disagree: disagree.iter().map(|d| d.to_string()).collect(),
        }
    }

    fn all(era: Tally, params: Tally, child: Tally) -> BTreeMap<LiveLocus, Tally> {
        BTreeMap::from([
            (LiveLocus::EraSummary, era),
            (LiveLocus::Params, params),
            (LiveLocus::ChildAsOutput, child),
        ])
    }

    #[test]
    fn agreeing_live_comparisons_pass_with_no_lines() {
        let settled = settle(
            &all(tally(1, &[]), tally(50, &[]), tally(12, &[])),
            &KnownLive::new(),
        );
        assert_eq!(
            settled,
            Settled {
                lines: vec![],
                known_fired: KnownLive::new(),
                pass: true
            }
        );
    }

    #[test]
    fn a_disagreement_off_the_list_fails_and_one_on_it_passes() {
        let tallies = all(
            tally(1, &["name=unknown node=Dijkstra"]),
            tally(50, &["costModels.PlutusV1[0] ledger=1 dolos=2"]),
            tally(12, &[]),
        );
        let known = KnownLive::from([(LiveLocus::EraSummary, Row::S16)]);
        let settled = settle(&tallies, &known);
        assert_eq!(
            settled.lines,
            vec![
                "KNOWN row=S16 locus=era_summary name=unknown node=Dijkstra",
                "LIVE-DIFF row=S15 locus=live_params costModels.PlutusV1[0] ledger=1 dolos=2",
            ]
        );
        assert!(!settled.pass);
        assert_eq!(settled.known_fired, known);
        let tallies = all(
            tally(1, &["name=unknown node=Dijkstra"]),
            tally(50, &[]),
            tally(12, &[]),
        );
        assert!(settle(&tallies, &known).pass);
    }

    #[test]
    fn a_known_locus_that_agrees_or_a_locus_that_compared_nothing_fails() {
        let known = KnownLive::from([(LiveLocus::EraSummary, Row::S16)]);
        let settled = settle(&all(tally(1, &[]), tally(50, &[]), tally(12, &[])), &known);
        assert_eq!(settled.lines, vec!["KNOWN_STALE row=S16 locus=era_summary"]);
        assert!(settled.known_fired.is_empty());
        assert!(!settled.pass);
        let mut tallies = all(tally(1, &[]), tally(50, &[]), tally(0, &[]));
        let settled = settle(&tallies, &KnownLive::new());
        assert_eq!(
            settled.lines,
            vec!["LIVE-GAP locus=live_as_output compared=0"]
        );
        assert!(!settled.pass);
        tallies.remove(&LiveLocus::Params);
        let settled = settle(&tallies, &KnownLive::new());
        assert_eq!(
            settled.lines,
            vec![
                "LIVE-GAP locus=live_params compared=0",
                "LIVE-GAP locus=live_as_output compared=0",
            ]
        );
    }

    #[test]
    fn the_last_served_era_name_is_the_node_era() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(era_name(&names(&["conway", "dijkstra"]), "Dijkstra").agrees());
        let unknown = era_name(&names(&["unknown"]), "Dijkstra");
        assert_eq!(unknown.to_string(), "name=unknown node=Dijkstra");
        assert_eq!(
            era_name(&names(&[]), "Dijkstra").to_string(),
            "name=none node=Dijkstra"
        );
    }
}
