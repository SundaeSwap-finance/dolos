use crate::serve::ChainReads;
use futures_core::Stream;
use futures_util::StreamExt;
use pallas::interop::utxorpc::v1alpha::{self as interop, spec as u5c};
use pallas::ledger::traverse::MultiEraBlock;
use std::pin::Pin;
use tonic::{Request, Response, Status};

use super::criterion;
use crate::prelude::*;
use crate::serve::grpc::block_context::BlockContext;
use crate::serve::grpc::masking::{BlockMask, TxMask};
use crate::serve::grpc::run_blocking;
use crate::serve::grpc::stream::ChainStream;

/// The transaction itself, the one body a tx pattern is checked against.
fn members(tx: &u5c::cardano::Tx) -> impl Iterator<Item = &u5c::cardano::Tx> {
    std::iter::once(tx)
}

crate::serve::grpc::tx_pattern::matchers!(u5c, criterion = criterion, members = members);

/// The transactions of a block the request admits, each built when read with
/// the parts its mask selects, the block mapped once and its bytes shared.
pub(super) fn block_to_txs<C: ChainReads>(
    block: &RawBlock,
    context: &C,
    request: &u5c::watch::WatchTxRequest,
) -> impl Iterator<Item = u5c::watch::AnyChainTx> + Send + 'static {
    let mask = TxMask::from_paths(
        request
            .field_mask
            .as_ref()
            .map(|m| m.paths.as_slice())
            .unwrap_or_default(),
    );
    let native_bytes = mask
        .block
        .native_bytes
        .then(|| bytes::Bytes::copy_from_slice(block.as_slice()));
    let block = MultiEraBlock::decode(block).unwrap();
    let mapper = interop::Mapper::new(BlockContext::new(context.clone(), &block));

    let txs: Vec<_> = block
        .txs()
        .iter()
        .map(|x: &pallas::ledger::traverse::MultiEraTx<'_>| mapper.map_tx(x))
        .filter(|tx| {
            request
                .predicate
                .as_ref()
                .is_none_or(|predicate| apply_predicate(predicate, tx))
        })
        .collect();
    let mapped = (!txs.is_empty() && mask.block.chain).then(|| mapper.map_block(&block));

    txs.into_iter().map(move |x| u5c::watch::AnyChainTx {
        chain: mask
            .tx
            .then_some(u5c::watch::any_chain_tx::Chain::Cardano(x)),
        block: (mask.block != BlockMask::none()).then(|| u5c::watch::AnyChainBlock {
            native_bytes: native_bytes.clone().unwrap_or_default(),
            chain: mapped
                .clone()
                .map(u5c::watch::any_chain_block::Chain::Cardano),
        }),
    })
}

fn raw_to_blockref(raw: &[u8]) -> u5c::watch::BlockRef {
    let block = MultiEraBlock::decode(raw).unwrap();
    u5c::watch::BlockRef {
        slot: block.slot(),
        hash: block.hash().to_vec().into(),
        height: block.number(),
    }
}

fn roll_to_watch_response<C: ChainReads>(
    context: &C,
    log: &TipEvent,
    request: &u5c::watch::WatchTxRequest,
) -> Box<dyn Iterator<Item = u5c::watch::WatchTxResponse> + Send> {
    match log {
        TipEvent::Apply(_, block) => {
            let mut txs = block_to_txs(block, context, request).peekable();
            if txs.peek().is_none() {
                Box::new(std::iter::once(u5c::watch::WatchTxResponse {
                    action: Some(u5c::watch::watch_tx_response::Action::Idle(
                        raw_to_blockref(block),
                    )),
                }))
            } else {
                Box::new(
                    txs.map(u5c::watch::watch_tx_response::Action::Apply)
                        .map(|x| u5c::watch::WatchTxResponse { action: Some(x) }),
                )
            }
        }
        TipEvent::Undo(_, block) => Box::new(
            block_to_txs(block, context, request)
                .map(u5c::watch::watch_tx_response::Action::Undo)
                .map(|x| u5c::watch::WatchTxResponse { action: Some(x) }),
        ),
        // TODO: shouldn't we have a u5c event for origin?
        TipEvent::Mark(..) => Box::new(std::iter::empty()),
    }
}

pub struct WatchServiceImpl<D, C>
where
    D: Domain + ChainReads,
    C: CancelToken,
{
    domain: D,
    cancel: C,
}

impl<D, C> WatchServiceImpl<D, C>
where
    D: Domain + ChainReads,
    C: CancelToken,
{
    pub fn new(domain: D, cancel: C) -> Self {
        Self { domain, cancel }
    }
}

#[async_trait::async_trait]
impl<D, C> u5c::watch::watch_service_server::WatchService for WatchServiceImpl<D, C>
where
    D: Domain + ChainReads,
    C: CancelToken,
{
    type WatchTxStream = Pin<
        Box<dyn Stream<Item = Result<u5c::watch::WatchTxResponse, tonic::Status>> + Send + 'static>,
    >;

    async fn watch_tx(
        &self,
        request: Request<u5c::watch::WatchTxRequest>,
    ) -> Result<Response<Self::WatchTxStream>, Status> {
        let inner_req = request.into_inner();

        let intersect = inner_req
            .intersect
            .iter()
            .map(|x| ChainPoint::Specific(x.slot, x.hash.to_vec().as_slice().into()))
            .collect::<Vec<ChainPoint>>();

        let stream =
            ChainStream::start::<D, _>(self.domain.clone(), intersect.clone(), self.cancel.clone())
                .await
                .map_err(|e| Status::internal(format!("failed to start chain stream: {e}")))?
                .ok_or_else(|| {
                    Status::not_found(format!(
                        "none of the requested points intersect with local history: {intersect:?}"
                    ))
                })?;

        let domain = self.domain.clone();
        let request = std::sync::Arc::new(inner_req);

        let stream = stream
            .then(move |log| {
                let domain = domain.clone();
                let request = request.clone();
                async move {
                    let log =
                        log.map_err(|e| Status::internal(format!("chain stream failed: {e}")))?;
                    run_blocking(&domain, move |domain| {
                        Ok(roll_to_watch_response(&domain, &log, &request))
                    })
                    .await
                }
            })
            .flat_map(|responses| match responses {
                Ok(responses) => tokio_stream::iter(responses.map(Ok)).left_stream(),
                Err(status) => {
                    futures_util::stream::once(async move { Err(status) }).right_stream()
                }
            });

        Ok(Response::new(Box::pin(stream)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transactions_of_one_block_share_its_bytes() {
        #[derive(Clone)]
        struct NoOutputs;
        impl pallas::interop::utxorpc::LedgerContext for NoOutputs {
            fn get_utxos(
                &self,
                _: &[pallas::interop::utxorpc::TxoRef],
            ) -> Option<pallas::interop::utxorpc::UtxoMap> {
                Some(Default::default())
            }
            fn get_slot_timestamp(&self, _: u64) -> Option<u64> {
                Some(42)
            }
        }
        let (blocks, _, _) = dolos_testing::synthetic::build_synthetic_blocks(
            dolos_testing::synthetic::SyntheticBlockConfig {
                block_count: 2,
                txs_per_block: 4,
                spend_previous_outputs: true,
                ..Default::default()
            },
        );
        let raw = blocks[1].clone();
        let request = u5c::watch::WatchTxRequest::default();
        let txs: Vec<_> = block_to_txs(&raw, &NoOutputs, &request).collect();
        assert_eq!(txs.len(), 4);
        let served: Vec<_> = txs
            .iter()
            .map(|tx| tx.block.as_ref().unwrap().native_bytes.clone())
            .collect();
        assert!(served.iter().all(|b| b.as_ref() == raw.as_slice()));
        assert!(served.iter().all(|b| b.as_ptr() == served[0].as_ptr()));
    }

    fn masked(paths: &[&str]) -> u5c::watch::WatchTxRequest {
        let mut request = u5c::watch::WatchTxRequest {
            field_mask: Some(Default::default()),
            ..Default::default()
        };
        request.field_mask.as_mut().unwrap().paths = paths.iter().map(|p| p.to_string()).collect();
        request
    }

    /// Each answer holds the parts of the transaction and its block the mask
    /// names, and every part when the request has no mask.
    #[test]
    fn each_answer_holds_what_the_mask_names() {
        let (blocks, _, _) = dolos_testing::synthetic::build_synthetic_blocks(
            dolos_testing::synthetic::SyntheticBlockConfig {
                block_count: 1,
                txs_per_block: 3,
                ..Default::default()
            },
        );
        let raw = blocks[0].clone();
        let context = dolos_testing::toy_domain::ToyDomain::new(None, None);

        let unmasked: Vec<_> =
            block_to_txs(&raw, &context, &u5c::watch::WatchTxRequest::default()).collect();
        assert_eq!(unmasked.len(), 3);
        for tx in &unmasked {
            let block = tx
                .block
                .as_ref()
                .expect("an unmasked answer holds its block");
            assert_eq!(block.native_bytes.as_ref(), raw.as_slice());
            assert!(block.chain.is_some());
            assert!(tx.chain.is_some());
        }

        let tx_only: Vec<_> =
            block_to_txs(&raw, &context, &masked(&["apply.cardano.hash"])).collect();
        assert_eq!(tx_only.len(), 3);
        for tx in &tx_only {
            assert_eq!(tx.block, None);
            assert!(tx.chain.is_some());
        }

        let bytes_only: Vec<_> =
            block_to_txs(&raw, &context, &masked(&["apply.block.native_bytes"])).collect();
        assert_eq!(bytes_only.len(), 3);
        for tx in &bytes_only {
            let block = tx.block.as_ref().expect("the mask names the block's bytes");
            assert_eq!(block.native_bytes.as_ref(), raw.as_slice());
            assert_eq!(block.chain, None);
            assert_eq!(tx.chain, None);
        }
    }
}
