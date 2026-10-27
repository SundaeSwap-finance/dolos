use std::{collections::BTreeMap, fmt, time::Duration};

use dolos_core::{BlockHash, ChainPoint};
use futures_util::StreamExt as _;
use ledger_oracle_judge::cbor::TxIn;
use tokio_util::sync::CancellationToken;
use tonic::Request;

use super::super::{
    spec::{
        cardano,
        sync::{self as s, sync_service_server::SyncService as _},
        watch::{self as w, watch_service_server::WatchService as _},
    },
    sync::SyncServiceImpl,
    watch::WatchServiceImpl,
};
use crate::{adapters::DomainAdapter, prelude::CancelTokenImpl};

/// How long a stream read waits for its next response.
const NEXT: Duration = Duration::from_secs(10);

/// A service that maps a transaction with its resolved inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Service {
    FetchBlock,
    FollowTip,
    WatchTx,
}

impl fmt::Display for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Service::FetchBlock => "FetchBlock",
            Service::FollowTip => "FollowTip",
            Service::WatchTx => "WatchTx",
        };
        f.write_str(name)
    }
}

/// The output bytes one service serves for each input a transaction names, by transaction hash.
pub(super) type Served = BTreeMap<[u8; 32], Vec<(TxIn, Option<Vec<u8>>)>>;

fn hash32(bytes: &[u8]) -> [u8; 32] {
    bytes.try_into().expect("a served hash is 32 bytes")
}

fn served_input(input: &cardano::TxInput) -> (TxIn, Option<Vec<u8>>) {
    let txin = TxIn {
        tx: hash32(&input.tx_hash),
        ix: input.output_index.into(),
    };
    let output = input.as_output.as_ref().map(|o| {
        let cbor = o.original_cbor.as_ref();
        cbor.expect("a served output carries its bytes").to_vec()
    });
    (txin, output)
}

fn add_tx(served: &mut Served, tx: &cardano::Tx) {
    let common = tx
        .common
        .as_ref()
        .expect("a served transaction has common fields");
    let collateral = tx.collateral.iter().flat_map(|c| &c.collateral);
    let subs = tx
        .sub_transactions
        .iter()
        .flat_map(|s| s.inputs.iter().chain(&s.reference_inputs));
    let inputs = common
        .inputs
        .iter()
        .chain(collateral)
        .chain(&common.reference_inputs)
        .chain(subs)
        .map(served_input)
        .collect();
    served.insert(hash32(&common.hash), inputs);
}

fn add_block(served: &mut Served, block: &s::AnyChainBlock) {
    let Some(s::any_chain_block::Chain::Cardano(block)) = &block.chain else {
        panic!("a served block is a Cardano block");
    };
    for tx in block.body.iter().flat_map(|b| &b.tx) {
        add_tx(served, tx);
    }
}

fn block_ref(point: &ChainPoint) -> s::BlockRef {
    s::BlockRef {
        slot: point.slot(),
        hash: point.hash().map(|h| h.to_vec()).unwrap_or_default().into(),
        ..Default::default()
    }
}

/// The inputs FetchBlock serves for the transactions of each block.
pub(super) async fn fetch_block(domain: &DomainAdapter, blocks: &[BlockHash]) -> Served {
    let service = SyncServiceImpl::new(domain.clone(), CancelTokenImpl::default());
    let mut served = Served::new();
    for hash in blocks {
        let request = s::FetchBlockRequest {
            r#ref: Some(s::BlockRef {
                hash: hash.to_vec().into(),
                ..Default::default()
            }),
            field_mask: None,
        };
        let response = service.fetch_block(Request::new(request)).await;
        let block = response.unwrap().into_inner().block;
        add_block(
            &mut served,
            &block.expect("FetchBlock serves a block it holds"),
        );
    }
    served
}

/// The inputs FollowTip serves for the transactions of the blocks after the point, reading until it has applied the given number of blocks.
pub(super) async fn follow_tip(domain: &DomainAdapter, from: &ChainPoint, blocks: usize) -> Served {
    let cancel = CancellationToken::new();
    let service = SyncServiceImpl::new(domain.clone(), CancelTokenImpl(cancel.clone()));
    let request = s::FollowTipRequest {
        intersect: vec![block_ref(from)],
        field_mask: None,
    };
    let response = service.follow_tip(Request::new(request)).await;
    let mut stream = response.unwrap().into_inner();
    let mut served = Served::new();
    let mut applied = 0;
    while applied < blocks {
        let Ok(Some(next)) = tokio::time::timeout(NEXT, stream.next()).await else {
            break;
        };
        match next.unwrap().action {
            Some(s::follow_tip_response::Action::Apply(block)) => {
                add_block(&mut served, &block);
                applied += 1;
            }
            Some(s::follow_tip_response::Action::Reset(_)) => {}
            other => panic!("FollowTip from a point it holds serves {other:?}"),
        }
    }
    cancel.cancel();
    served
}

/// The inputs WatchTx serves for the transactions after the point, reading until it has applied the given number of transactions.
pub(super) async fn watch_tx(domain: &DomainAdapter, from: &ChainPoint, txs: usize) -> Served {
    let cancel = CancellationToken::new();
    let service = WatchServiceImpl::new(domain.clone(), CancelTokenImpl(cancel.clone()));
    let from = block_ref(from);
    let request = w::WatchTxRequest {
        intersect: vec![w::BlockRef {
            slot: from.slot,
            hash: from.hash,
            height: 0,
        }],
        predicate: None,
        field_mask: None,
    };
    let response = service.watch_tx(Request::new(request)).await;
    let mut stream = response.unwrap().into_inner();
    let mut served = Served::new();
    let mut applied = 0;
    while applied < txs {
        let Ok(Some(next)) = tokio::time::timeout(NEXT, stream.next()).await else {
            break;
        };
        match next.unwrap().action {
            Some(w::watch_tx_response::Action::Apply(tx)) => {
                let Some(w::any_chain_tx::Chain::Cardano(tx)) = &tx.chain else {
                    panic!("a watched transaction is a Cardano transaction");
                };
                add_tx(&mut served, tx);
                applied += 1;
            }
            other => panic!("WatchTx over blocks that hold transactions serves {other:?}"),
        }
    }
    cancel.cancel();
    served
}
