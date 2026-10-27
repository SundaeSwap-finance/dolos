use std::fmt;

use serde::{de::Error as _, Deserialize, Deserializer};

use crate::cbor::{self, TxIn};

/// The corpus a record comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CorpusKind {
    Ic,
    Supp,
}

impl CorpusKind {
    /// The short name used in record ids and printed lines.
    pub fn name(self) -> &'static str {
        match self {
            CorpusKind::Ic => "ic",
            CorpusKind::Supp => "supp",
        }
    }
}

/// One record of one corpus, printed as `ic#851`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordId {
    pub corpus: CorpusKind,
    pub n: u32,
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.corpus.name(), self.n)
    }
}

/// Bytes written as a hex string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bytes(pub Vec<u8>);

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        hex::decode(&s).map(Bytes).map_err(D::Error::custom)
    }
}

impl<'de> Deserialize<'de> for TxIn {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Bytes(raw) = Bytes::deserialize(d)?;
        cbor::txin(&raw).map_err(D::Error::custom)
    }
}

/// A list with at least one element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmpty<T> {
    pub head: T,
    pub tail: Vec<T>,
}

impl<T> NonEmpty<T> {
    /// The elements in order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        std::iter::once(&self.head).chain(&self.tail)
    }

    /// The final element.
    pub fn last(&self) -> &T {
        self.tail.last().unwrap_or(&self.head)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for NonEmpty<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut all = Vec::<T>::deserialize(d)?.into_iter();
        let head = all
            .next()
            .ok_or_else(|| D::Error::invalid_length(0, &"at least one element"))?;
        Ok(NonEmpty {
            head,
            tail: all.collect(),
        })
    }
}

/// The constructor names of one ledger failure, from the outer wrapper to the leaf.
pub type FailurePath = NonEmpty<String>;

/// The protocol parameters the ledger ran a record under.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PParams(pub serde_json::Map<String, serde_json::Value>);

/// Whether a transaction declares its scripts to pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(from = "bool")]
pub enum Phase2 {
    Valid,
    Invalid,
}

impl From<bool> for Phase2 {
    fn from(valid: bool) -> Self {
        if valid {
            Phase2::Valid
        } else {
            Phase2::Invalid
        }
    }
}

/// One unspent output, its input key and its output bytes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "(TxIn, Bytes)")]
pub struct UtxoEntry {
    pub input: TxIn,
    pub output: Bytes,
}

impl From<(TxIn, Bytes)> for UtxoEntry {
    fn from((input, output): (TxIn, Bytes)) -> Self {
        UtxoEntry { input, output }
    }
}

/// The ledger's answer for one record.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum LedgerOutcome {
    Accepted {
        post_utxo: Vec<UtxoEntry>,
        accounts_post: serde_json::Value,
    },
    Refused {
        failures: String,
        failure_names: NonEmpty<FailurePath>,
    },
}

/// The chain values the ledger ran the corpus under, written on its first record.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Globals {
    pub network: String,
    pub epoch_size: u64,
    pub security_param: u64,
}

/// One LEDGER rule run as the extractor wrote it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub n: u32,
    pub slot: u64,
    pub epoch: Option<u64>,
    pub follows_prev: bool,
    pub phase2_valid: Phase2,
    pub tx: Bytes,
    pub pparams: PParams,
    pub pre_utxo: Vec<UtxoEntry>,
    pub accounts_pre: serde_json::Value,
    pub outcome: LedgerOutcome,
    #[serde(default)]
    pub globals: Option<Globals>,
}
