//! Judges the fee each ReadTx version maps for a transaction against the ledger's transaction.

use std::{fmt, panic::AssertUnwindSafe};

use futures_util::FutureExt as _;
use itertools::Itertools as _;
use ledger_oracle_judge::{
    compare::Tally,
    eval::{Layer, WireFee},
    record::{CorpusKind, LedgerOutcome, RecordId},
};
use pallas::ledger::traverse::{Era, MultiEraTx};
use tonic::Request;

use super::{eval::wire_fee, validate::panic_message, GroupBlock, TxName};
use crate::adapters::DomainAdapter;

/// The fee of a transaction as the ledger holds it.
struct LedgerTx {
    fee: Option<u64>,
}

impl LedgerTx {
    /// The fields the ledger holds for a transaction.
    fn of(tx: &MultiEraTx<'_>) -> Self {
        LedgerTx { fee: tx.fee() }
    }
}

/// The fee a ReadTx version maps for a transaction, with no fee in a message that has no fee field.
struct MappedTx {
    fee: Option<WireFee>,
}

/// What a ReadTx version answers for a transaction hash.
enum Answer<T> {
    /// The mapped transaction.
    Mapped(T),
    /// An answer that holds no transaction where the ledger names one.
    Absent,
    /// The status the version answers in place of a transaction.
    Status(tonic::Code),
    /// The message of a panic in the version's mapping.
    Panicked(String),
}

impl<T> Answer<T> {
    /// The answer for a part of the mapped transaction, or this answer's absence, status or panic.
    fn then<U>(&self, part: impl FnOnce(&T) -> Answer<U>) -> Answer<U> {
        match self {
            Answer::Mapped(tx) => part(tx),
            Answer::Absent => Answer::Absent,
            Answer::Status(code) => Answer::Status(*code),
            Answer::Panicked(message) => Answer::Panicked(message.clone()),
        }
    }
}

/// The ReadTx answer of one u5c version for a transaction hash against the domain, with a panic of its mapping caught.
macro_rules! read_tx {
    ($version:ident, $domain:expr, $hash:expr) => {{
        use pallas::interop::utxorpc::$version::spec::query::{
            any_chain_tx::Chain, query_service_server::QueryService as _, ReadTxRequest,
        };
        let request = ReadTxRequest {
            hash: $hash.to_vec().into(),
            ..Default::default()
        };
        let service = crate::serve::grpc::$version::query::QueryServiceImpl::new($domain.clone());
        let answer = AssertUnwindSafe(service.read_tx(Request::new(request)))
            .catch_unwind()
            .await;
        match answer {
            Err(payload) => Answer::Panicked(panic_message(payload).replace('\n', " ")),
            Ok(Err(status)) => Answer::Status(status.code()),
            Ok(Ok(answer)) => match answer.into_inner().tx.and_then(|tx| tx.chain) {
                Some(Chain::Cardano(tx)) => Answer::Mapped(tx),
                None => Answer::Absent,
            },
        }
    }};
}

/// A field the judge compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Fee,
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Field::Fee => "fee",
        })
    }
}

/// Each field the mapping holds a different value in than the ledger, with the two values of each.
fn differing(ledger: &LedgerTx, mapped: &MappedTx) -> Vec<(Field, String, String)> {
    let mut differing = vec![];
    if let Some(fee) = &mapped.fee {
        let holds = match (ledger.fee, fee) {
            (None, WireFee::Unset) => true,
            (Some(n), WireFee::Int(m)) => i64::try_from(n) == Ok(*m),
            _ => false,
        };
        if !holds {
            differing.push((Field::Fee, format!("{:?}", ledger.fee), format!("{fee:?}")));
        }
    }
    differing
}

/// Adds the comparison of what one version answers for one transaction the ledger accepted.
fn add(
    tally: &mut Tally,
    id: RecordId,
    layer: Layer,
    name: TxName,
    ledger: &LedgerTx,
    answer: Answer<MappedTx>,
) {
    let (agrees, dolos) = match answer {
        Answer::Mapped(mapped) => {
            let differing = differing(ledger, &mapped);
            let fields = differing.iter().map(|(f, _, _)| f).join(",");
            let mut ledger = differing.iter().map(|(f, l, _)| format!("{f}={l}"));
            let ledger = ledger.join(" ");
            let mut dolos = differing.iter().map(|(f, _, d)| format!("{f}={d}"));
            let dolos = dolos.join(" ");
            let shown = format!("differing={fields} ledger=[{ledger}] dolos=[{dolos}]");
            (differing.is_empty(), shown)
        }
        Answer::Absent => (false, "dolos=none".to_string()),
        Answer::Status(code) => (false, format!("status={code:?}")),
        Answer::Panicked(message) => (false, format!("panicked={message}")),
    };
    tally.add(agrees, || format!("{id} {layer} {name} {dolos}"));
}

/// Whether the v1 mapping of a run's transactions panicked, which leaves the passes that map them through v1 unrun.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum V1Mapping {
    Maps,
    Panics,
}

/// Counts what each ReadTx version maps for the accepted records of the blocks a run rolled against the ledger's transactions, v1 by each top level hash with its sub transactions and v1beta and v1alpha by each top level and sub transaction hash.
pub(super) async fn compare(
    domain: &DomainAdapter,
    kind: CorpusKind,
    blocks: &[GroupBlock<'_>],
    tally: &mut Tally,
) -> V1Mapping {
    let mut v1 = V1Mapping::Maps;
    let groups = blocks.iter().filter_map(GroupBlock::rolled);
    let accepted = groups
        .flat_map(|rolled| rolled.group)
        .filter(|r| matches!(r.outcome, LedgerOutcome::Accepted { .. }));
    for record in accepted {
        let id = RecordId {
            corpus: kind,
            n: record.n,
        };
        let tx = MultiEraTx::decode_for_era(Era::Dijkstra, &record.tx.0)
            .unwrap_or_else(|e| panic!("TX {id} {e}"));
        let top = (*tx.hash(), LedgerTx::of(&tx));
        let subs = tx.sub_transactions();
        let subs: Vec<([u8; 32], LedgerTx)> = subs
            .iter()
            .map(|sub| (*sub.hash(), LedgerTx::of(sub)))
            .collect();
        let served = read_tx!(v1, domain, top.0);
        if let Answer::Panicked(_) = served {
            v1 = V1Mapping::Panics;
        }
        let common = served.then(|tx| {
            let fee = wire_fee!(v1, tx.fee.clone());
            match &tx.common {
                Some(_) => Answer::Mapped(MappedTx { fee: Some(fee) }),
                None => Answer::Absent,
            }
        });
        add(tally, id, Layer::V1, TxName::Top(top.0), &top.1, common);
        for (hash, ledger) in &subs {
            let common = served.then(|tx| {
                let sub = tx.sub_transactions.iter().find(|c| c.hash == hash[..]);
                match sub {
                    Some(_) => Answer::Mapped(MappedTx { fee: None }),
                    None => Answer::Absent,
                }
            });
            add(tally, id, Layer::V1, TxName::Sub(*hash), ledger, common);
        }
        let names = std::iter::once((TxName::Top(top.0), &top.1));
        let names = names.chain(subs.iter().map(|(hash, l)| (TxName::Sub(*hash), l)));
        for (name, ledger) in names {
            let hash = match name {
                TxName::Top(h) | TxName::Sub(h) => h,
            };
            let answer = read_tx!(v1beta, domain, hash).then(|tx| {
                let fee = wire_fee!(v1beta, tx.fee.clone());
                Answer::Mapped(MappedTx { fee: Some(fee) })
            });
            add(tally, id, Layer::V1beta, name, ledger, answer);
            let answer = read_tx!(v1alpha, domain, hash).then(|tx| {
                let fee = wire_fee!(v1alpha, tx.fee.clone());
                Answer::Mapped(MappedTx { fee: Some(fee) })
            });
            add(tally, id, Layer::V1alpha, name, ledger, answer);
        }
    }
    v1
}

fn ledger(fee: Option<u64>) -> LedgerTx {
    LedgerTx { fee }
}

fn mapped(fee: Option<WireFee>) -> MappedTx {
    MappedTx { fee }
}

fn fields(ledger: &LedgerTx, mapped: &MappedTx) -> Vec<Field> {
    let differing = differing(ledger, mapped);
    differing.into_iter().map(|d| d.0).collect()
}

#[test]
fn a_fee_the_ledger_holds_none_of_differs_when_mapped_as_zero() {
    let none: Vec<Field> = vec![];
    let (unset, zero) = (mapped(Some(WireFee::Unset)), mapped(Some(WireFee::Int(0))));
    assert_eq!(fields(&ledger(None), &unset), none);
    let fee = Some(WireFee::Int(170_000));
    assert_eq!(fields(&ledger(Some(170_000)), &mapped(fee)), none);
    assert_eq!(fields(&ledger(Some(7)), &mapped(None)), none);
    assert_eq!(fields(&ledger(None), &zero), [Field::Fee]);
    assert_eq!(fields(&ledger(Some(7)), &unset), [Field::Fee]);
}
