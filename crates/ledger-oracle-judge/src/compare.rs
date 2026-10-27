use std::fmt;

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
}

impl StateTallies {
    fn named(&self) -> [(&'static str, &Tally); 1] {
        [("utxo", &self.utxo)]
    }

    /// Whether every comparison looked at something and agreed on all of it.
    pub fn pass(&self) -> bool {
        self.named()
            .iter()
            .all(|(_, t)| t.compared > 0 && t.disagree.is_empty())
    }
}

impl fmt::Display for StateTallies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "STATE compared")?;
        for (name, t) in self.named() {
            write!(f, " {name}={}", t.compared)?;
        }
        for (name, t) in self.named() {
            if !t.disagree.is_empty() {
                write!(f, "\nSTATE {name} disagree={}", t.disagree.len())?;
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

    #[test]
    fn byte_verdict_prints_each_side() {
        assert_eq!(ByteVerdict::of(Some(b"\x01"), Some(b"\x01")), Some(ByteVerdict::Equal));
        assert_eq!(
            ByteVerdict::of(Some(b"\x01\x02"), Some(b"\x01\x03")).unwrap().to_string(),
            "differ at byte 1"
        );
        assert_eq!(
            ByteVerdict::of(Some(b"\xab"), None).unwrap().to_string(),
            "ledger=ab dolos=none"
        );
        assert_eq!(ByteVerdict::of(None, None), None);
    }
}
