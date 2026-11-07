//! Judges that ReadTx v1 maps each transaction the ledger accepted, top level and sub transaction.

use std::panic::AssertUnwindSafe;

use futures_util::FutureExt as _;
use ledger_oracle_judge::{
    compare::Tally,
    eval::Layer,
    record::{CorpusKind, LedgerOutcome, RecordId},
};
use pallas::ledger::traverse::{Era, MultiEraTx};
use tonic::Request;

use super::{validate::panic_message, GroupBlock, TxName};
use crate::adapters::DomainAdapter;

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

/// Adds the comparison of what one version answers for one transaction the ledger accepted.
fn add(tally: &mut Tally, id: RecordId, layer: Layer, name: TxName, answer: Answer<()>) {
    let (agrees, dolos) = match answer {
        Answer::Mapped(()) => (true, String::new()),
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

/// Counts whether ReadTx v1 maps each accepted record of the blocks a run rolled, by its top level hash with its sub transactions.
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
        let top = *tx.hash();
        let subs = tx.sub_transactions();
        let subs: Vec<[u8; 32]> = subs.iter().map(|sub| *sub.hash()).collect();
        let served = read_tx!(v1, domain, top);
        if let Answer::Panicked(_) = served {
            v1 = V1Mapping::Panics;
        }
        let common = served.then(|tx| match &tx.common {
            Some(_) => Answer::Mapped(()),
            None => Answer::Absent,
        });
        add(tally, id, Layer::V1, TxName::Top(top), common);
        for hash in &subs {
            let common = served.then(|tx| {
                let sub = tx.sub_transactions.iter().find(|c| c.hash == hash[..]);
                match sub {
                    Some(_) => Answer::Mapped(()),
                    None => Answer::Absent,
                }
            });
            add(tally, id, Layer::V1, TxName::Sub(*hash), common);
        }
    }
    v1
}
