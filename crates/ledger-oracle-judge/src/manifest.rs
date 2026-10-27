use std::{collections::BTreeMap, fmt, str::FromStr};

use thiserror::Error;

use crate::record::{CorpusKind, RecordId};

const EXPECTED: &str = include_str!("../../../test_data/ledger-oracle/expected-unsupported.txt");
const KNOWN: &str = include_str!("../../../test_data/ledger-oracle/known.txt");

/// An unsupported feature label as Dolos's validator names it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Feature(pub String);

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The comparison a known disagreement shows in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Locus {
    Validation,
}

impl FromStr for Locus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "validation" => Ok(Locus::Validation),
            other => Err(format!("unknown locus {other}")),
        }
    }
}

/// A manifest line that does not parse.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("MANIFEST {file} line={line} {reason}")]
pub struct ManifestError {
    pub file: &'static str,
    pub line: usize,
    pub reason: String,
}

impl FromStr for RecordId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (corpus, n) = s.split_once('#').ok_or(format!("bad record id {s}"))?;
        let corpus = match corpus {
            "ic" => CorpusKind::Ic,
            "supp" => CorpusKind::Supp,
            other => return Err(format!("unknown corpus {other}")),
        };
        let n = n.parse().map_err(|_| format!("bad record number {n}"))?;
        Ok(RecordId { corpus, n })
    }
}

/// The feature each record is declared to be refused as unsupported for.
pub type Declared = BTreeMap<RecordId, Feature>;

/// The catalog row each known disagreement is filed under, by locus and record.
pub type KnownList = BTreeMap<(Locus, RecordId), String>;

fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty())
}

/// Reads declared support, one `<record> <label>` per line.
pub fn parse_declared(file: &'static str, text: &str) -> Result<Declared, ManifestError> {
    let mut declared = Declared::new();
    for (line, l) in lines(text) {
        let error = |reason: String| ManifestError { file, line, reason };
        let (id, label) = l.split_once(' ').ok_or(error("no label".into()))?;
        let id: RecordId = id.parse().map_err(error)?;
        if declared.insert(id, Feature(label.to_string())).is_some() {
            return Err(error(format!("{id} declared twice")));
        }
    }
    Ok(declared)
}

/// Reads the known list, one `<row> <locus> <record>...` per line.
pub fn parse_known(file: &'static str, text: &str) -> Result<KnownList, ManifestError> {
    let mut known = KnownList::new();
    for (line, l) in lines(text) {
        let error = |reason: String| ManifestError { file, line, reason };
        let mut words = l.split(' ');
        let row = words.next().unwrap_or_default();
        let locus: Locus = words
            .next()
            .ok_or(error("no locus".into()))?
            .parse()
            .map_err(error)?;
        let mut ids = 0;
        for id in words {
            let id: RecordId = id.parse().map_err(error)?;
            if known.insert((locus, id), row.to_string()).is_some() {
                return Err(error(format!("{id} known twice")));
            }
            ids += 1;
        }
        if ids == 0 {
            return Err(error("no record".into()));
        }
    }
    Ok(known)
}

/// The committed declared support.
pub fn declared() -> Result<Declared, ManifestError> {
    parse_declared("expected-unsupported.txt", EXPECTED)
}

/// The committed known list.
pub fn known() -> Result<KnownList, ManifestError> {
    parse_known("known.txt", KNOWN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_manifests_parse() {
        assert!(!declared().unwrap().is_empty());
        assert!(!known().unwrap().is_empty());
    }

    #[test]
    fn a_record_on_two_known_rows_fails_the_load() {
        assert_eq!(
            parse_known("k", "V1 validation supp#5\nV2 validation supp#5").unwrap_err(),
            ManifestError {
                file: "k",
                line: 2,
                reason: "supp#5 known twice".into()
            }
        );
    }

    #[test]
    fn a_label_keeps_its_spaces() {
        let declared = parse_declared("e", "ic#851 non-vkey witnesses").unwrap();
        assert_eq!(
            declared[&"ic#851".parse().unwrap()],
            Feature("non-vkey witnesses".into())
        );
    }
}
