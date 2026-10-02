use futures_core::Stream;
use futures_util::StreamExt;
use itertools::Itertools;
use pallas::interop::utxorpc::v1beta::spec::sync::BlockRef;
use pallas::interop::utxorpc::v1beta::{spec as u5c, Mapper};
use pallas::interop::utxorpc::LedgerContext;
use std::pin::Pin;
use tonic::{Request, Response, Status};

use crate::prelude::*;
use crate::serve::grpc::block_context::BlockContext;
use crate::serve::grpc::masking::BlockMask;
use pallas::ledger::traverse::MultiEraBlock;

const MAX_DUMP_HISTORY_ITEMS: u32 = 100;

fn u5c_to_chain_point(block_ref: u5c::sync::BlockRef) -> Result<ChainPoint, Status> {
    Ok(ChainPoint::Specific(
        block_ref.slot,
        crate::serve::grpc::convert::bytes_to_hash32(&block_ref.hash)?,
    ))
}

fn raw_to_anychain<C: LedgerContext>(
    context: &C,
    body: &BlockBody,
    mask: BlockMask,
) -> u5c::sync::AnyChainBlock {
    u5c::sync::AnyChainBlock {
        native_bytes: if mask.native_bytes {
            body.to_vec().into()
        } else {
            Default::default()
        },
        chain: if mask.chain {
            let block = MultiEraBlock::decode(body).unwrap();
            let prepared = BlockContext::new(context.clone(), &block);
            u5c::sync::any_chain_block::Chain::Cardano(Mapper::new(prepared).map_block(&block))
                .into()
        } else {
            None
        },
    }
}

fn raw_to_blockref<C: LedgerContext>(context: &C, body: &BlockBody) -> Option<u5c::sync::BlockRef> {
    // A tip reference needs no transaction mapping or input resolution.
    let block = MultiEraBlock::decode(body).unwrap();
    Some(u5c::sync::BlockRef {
        slot: block.slot(),
        hash: block.hash().to_vec().into(),
        height: block.number(),
        timestamp: context
            .get_slot_timestamp(block.slot())
            .map(|s| s * 1000)
            .unwrap_or(0),
    })
}

fn point_to_blockref(point: &ChainPoint, timestamp: u64) -> u5c::sync::BlockRef {
    BlockRef {
        hash: point.hash().map(|h| h.to_vec()).unwrap_or_default().into(),
        slot: point.slot(),
        timestamp,
        ..Default::default()
    }
}

fn tip_event_to_response<C: LedgerContext>(
    context: &C,
    event: &TipEvent,
    mask: BlockMask,
) -> u5c::sync::FollowTipResponse {
    match event {
        TipEvent::Apply(_, block) => {
            let block_ref = raw_to_blockref(context, block);
            u5c::sync::FollowTipResponse {
                action: Some(u5c::sync::follow_tip_response::Action::Apply(
                    raw_to_anychain(context, block, mask),
                )),
                tip: block_ref,
            }
        }
        TipEvent::Undo(_, block) => u5c::sync::FollowTipResponse {
            action: Some(u5c::sync::follow_tip_response::Action::Undo(
                raw_to_anychain(context, block, mask),
            )),
            tip: None, // TODO: we don't have easy access to the new tip here
        },
        TipEvent::Mark(x) => u5c::sync::FollowTipResponse {
            action: Some(u5c::sync::follow_tip_response::Action::Reset(
                point_to_blockref(x, 0), // TODO: we don't have the timestamp here
            )),
            tip: Some(point_to_blockref(x, 0)),
        },
    }
}

pub struct SyncServiceImpl<D, C>
where
    D: Domain + LedgerContext,
    C: CancelToken,
{
    domain: D,
    cancel: C,
}

impl<D, C> SyncServiceImpl<D, C>
where
    D: Domain + LedgerContext,
    C: CancelToken,
{
    pub fn new(domain: D, cancel: C) -> Self {
        Self { domain, cancel }
    }
}

#[async_trait::async_trait]
impl<D, C> u5c::sync::sync_service_server::SyncService for SyncServiceImpl<D, C>
where
    D: Domain + LedgerContext,
    C: CancelToken,
{
    type FollowTipStream =
        Pin<Box<dyn Stream<Item = Result<u5c::sync::FollowTipResponse, Status>> + Send + 'static>>;

    async fn fetch_block(
        &self,
        request: Request<u5c::sync::FetchBlockRequest>,
    ) -> Result<Response<u5c::sync::FetchBlockResponse>, Status> {
        let message = request.into_inner();

        let mask = BlockMask::from_paths(
            message
                .field_mask
                .as_ref()
                .map(|m| m.paths.as_slice())
                .unwrap_or_default(),
        );

        let query = dolos_core::AsyncQueryFacade::new(self.domain.clone());

        let mut out = Vec::new();
        for br in message.r#ref.iter() {
            let mut body: Option<BlockBody> = None;

            if !br.hash.is_empty() {
                body = query
                    .block_by_hash(br.hash.to_vec())
                    .await
                    .map_err(|_| Status::internal("Failed to query chain service."))?;
            }

            if body.is_none() && br.height != 0 {
                body = query
                    .block_by_number(br.height)
                    .await
                    .map_err(|_| Status::internal("Failed to query chain service."))?;
            }

            if body.is_none() && br.slot != 0 {
                body = query
                    .block_by_slot(br.slot)
                    .await
                    .map_err(|_| Status::internal("Failed to query chain service."))?;
            }

            let Some(body) = body else {
                return Err(Status::not_found(format!("Failed to find block: {br:?}")));
            };

            out.push(raw_to_anychain(&self.domain, &body, mask));
        }

        let response = u5c::sync::FetchBlockResponse { block: out };

        Ok(Response::new(response))
    }

    async fn dump_history(
        &self,
        request: Request<u5c::sync::DumpHistoryRequest>,
    ) -> Result<Response<u5c::sync::DumpHistoryResponse>, Status> {
        let msg = request.into_inner();

        let mask = BlockMask::from_paths(
            msg.field_mask
                .as_ref()
                .map(|m| m.paths.as_slice())
                .unwrap_or_default(),
        );

        let mut from = None;

        if let Some(ref br) = msg.start_token {
            let mut slot: Option<u64> = None;

            if !br.hash.is_empty() {
                slot = self
                    .domain
                    .archive()
                    .slot_by_block_hash(&br.hash)
                    .map_err(|_| Status::internal("Failed to query chain service."))?;
            }

            if slot.is_none() && br.height != 0 {
                slot = self
                    .domain
                    .archive()
                    .slot_by_block_number(br.height)
                    .map_err(|_| Status::internal("Failed to query chain service."))?;
            }

            if slot.is_none() && br.slot != 0 {
                slot = Some(br.slot);
            }

            from = match slot {
                Some(s) => Some(s),
                None if !br.hash.is_empty() || br.height != 0 || br.slot != 0 => {
                    return Err(Status::not_found(format!(
                        "Failed to find block for start_token: {br:?}"
                    )));
                }
                None => None,
            };
        }

        if msg.max_items > MAX_DUMP_HISTORY_ITEMS {
            return Err(Status::invalid_argument(format!(
                "max_items must be less than or equal to {MAX_DUMP_HISTORY_ITEMS}"
            )));
        }

        let len = msg.max_items as usize;

        let mut range = self
            .domain
            .archive()
            .get_range(from, None)
            .map_err(|_| Status::internal("cant query archive"))?;

        let items = range
            .by_ref()
            .take(len)
            .map(|(_, body)| raw_to_anychain(&self.domain, &body, mask))
            .collect();

        let next_token = range
            .next()
            .and_then(|(_, body)| raw_to_blockref(&self.domain, &body));

        let response = u5c::sync::DumpHistoryResponse {
            block: items,
            next_token,
        };

        Ok(Response::new(response))
    }

    async fn follow_tip(
        &self,
        request: Request<u5c::sync::FollowTipRequest>,
    ) -> Result<Response<Self::FollowTipStream>, tonic::Status> {
        let request = request.into_inner();

        let mask = BlockMask::from_paths(
            request
                .field_mask
                .as_ref()
                .map(|m| m.paths.as_slice())
                .unwrap_or_default(),
        );

        let intersect: Vec<_> = request
            .intersect
            .into_iter()
            .map(u5c_to_chain_point)
            .try_collect()?;

        let stream = crate::serve::grpc::stream::ChainStream::start::<D, _>(
            self.domain.clone(),
            intersect.clone(),
            self.cancel.clone(),
        )
        .map_err(|e| Status::internal(format!("failed to start chain stream: {e}")))?
        .ok_or_else(|| {
            Status::not_found(format!(
                "none of the requested points intersect with local history: {intersect:?}"
            ))
        })?;

        let context = self.domain.clone();

        let stream = stream.map(move |log| {
            let log = log.map_err(|e| Status::internal(format!("chain stream failed: {e}")))?;
            Ok(tip_event_to_response(&context, &log, mask))
        });

        Ok(Response::new(Box::pin(stream)))
    }

    async fn read_tip(
        &self,
        _request: tonic::Request<u5c::sync::ReadTipRequest>,
    ) -> std::result::Result<tonic::Response<u5c::sync::ReadTipResponse>, tonic::Status> {
        let (point, _) = self
            .domain
            .wal()
            .find_tip()
            .map_err(|e| Status::internal(format!("Unable to read WAL: {e:?}")))?
            .ok_or(Status::internal("chain has no data."))?;

        let timestamp = self
            .domain
            .get_slot_timestamp(point.slot())
            .map(|s| s * 1000)
            .unwrap_or(0);
        let response = u5c::sync::ReadTipResponse {
            tip: Some(point_to_blockref(&point, timestamp)),
        };

        Ok(Response::new(response))
    }
}

#[cfg(test)]
mod tests {
    use dolos_testing::toy_domain::ToyDomain;
    use pallas::interop::utxorpc::v1beta::spec::sync::sync_service_server::SyncService as _;

    use super::*;

    #[derive(Clone)]
    struct HeaderOnlyContext;

    impl LedgerContext for HeaderOnlyContext {
        fn get_utxos(
            &self,
            _: &[pallas::interop::utxorpc::TxoRef],
        ) -> Option<pallas::interop::utxorpc::UtxoMap> {
            panic!("native-only responses and tip references must not resolve inputs");
        }
        fn get_slot_timestamp(&self, _: u64) -> Option<u64> {
            Some(42)
        }
    }

    #[test]
    fn native_only_follow_tip_skips_input_resolution() {
        let (blocks, _, _) = dolos_testing::synthetic::build_synthetic_blocks(
            dolos_testing::synthetic::SyntheticBlockConfig {
                block_count: 2,
                txs_per_block: 4,
                spend_previous_outputs: true,
                ..Default::default()
            },
        );
        let raw = blocks[1].clone();
        let block = MultiEraBlock::decode(&raw).unwrap();
        let point = ChainPoint::Specific(block.slot(), block.hash());
        let response = tip_event_to_response(
            &HeaderOnlyContext,
            &TipEvent::Apply(point.clone(), raw.clone()),
            BlockMask::from_paths(&["native_bytes".to_owned()]),
        );
        let tip = response.tip.unwrap();
        assert_eq!(tip.slot, point.slot());
        assert_eq!(tip.timestamp, 42_000);
        match response.action.unwrap() {
            u5c::sync::follow_tip_response::Action::Apply(block) => {
                assert_eq!(block.native_bytes.as_ref(), raw.as_slice());
                assert!(block.chain.is_none());
            }
            _ => panic!("expected apply"),
        }
    }
    #[test]
    fn structured_follow_tip_resolves_once_and_preserves_output() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        #[derive(Clone, Default)]
        struct CountingContext(Arc<AtomicUsize>);
        impl LedgerContext for CountingContext {
            fn get_utxos(
                &self,
                _: &[pallas::interop::utxorpc::TxoRef],
            ) -> Option<pallas::interop::utxorpc::UtxoMap> {
                self.0.fetch_add(1, Ordering::Relaxed);
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
        let block = MultiEraBlock::decode(&raw).unwrap();
        let point = ChainPoint::Specific(block.slot(), block.hash());
        let context = CountingContext::default();
        let expected = Mapper::new(context.clone()).map_block(&block);
        let header = expected.header.clone().unwrap();
        context.0.store(0, Ordering::Relaxed);
        let response = tip_event_to_response(
            &context,
            &TipEvent::Apply(point, raw.clone()),
            BlockMask::from_paths(&[]),
        );
        let tip = response.tip.unwrap();
        assert_eq!(
            (tip.slot, tip.hash, tip.height, tip.timestamp),
            (header.slot, header.hash, header.height, expected.timestamp)
        );
        match response.action.unwrap() {
            u5c::sync::follow_tip_response::Action::Apply(result) => {
                assert_eq!(result.native_bytes.as_ref(), raw.as_slice());
                assert_eq!(
                    result.chain,
                    Some(u5c::sync::any_chain_block::Chain::Cardano(expected))
                );
            }
            _ => panic!("expected apply"),
        }
        assert_eq!(context.0.load(Ordering::Relaxed), 1);
        // Pagination's next token takes the same cheap header-only path.
        let _ = raw_to_blockref(&context, &raw).unwrap();
        assert_eq!(context.0.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_dump_history_pagination() {
        let domain = ToyDomain::new(None, None);
        let cancel = CancelTokenImpl::default();

        let batch = (0..34)
            .map(|i| dolos_testing::blocks::make_conway_block(i).1)
            .collect_vec();

        use dolos_core::ImportExt;
        domain.import_blocks(batch).unwrap();

        let service = SyncServiceImpl::new(domain, cancel);

        let mut start_token = None;

        for _ in 0..3 {
            let request = u5c::sync::DumpHistoryRequest {
                start_token,
                max_items: 10,
                field_mask: None,
            };

            let response = service
                .dump_history(Request::new(request))
                .await
                .unwrap()
                .into_inner();

            assert_eq!(response.block.len(), 10);

            start_token = response.next_token;
        }

        let request = u5c::sync::DumpHistoryRequest {
            start_token,
            max_items: 10,
            field_mask: None,
        };

        let response = service
            .dump_history(Request::new(request))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(response.block.len(), 4);
        assert_eq!(response.next_token, None);
    }

    #[tokio::test]
    async fn dump_history_applies_field_mask() {
        let domain = ToyDomain::new(None, None);
        let cancel = CancelTokenImpl::default();

        let batch = (0..3)
            .map(|i| dolos_testing::blocks::make_conway_block(i).1)
            .collect_vec();

        use dolos_core::ImportExt;
        domain.import_blocks(batch).unwrap();

        let service = SyncServiceImpl::new(domain, cancel);

        let mut request = u5c::sync::DumpHistoryRequest {
            start_token: None,
            max_items: 10,
            field_mask: Some(Default::default()),
        };
        request.field_mask.as_mut().unwrap().paths = vec!["block.native_bytes".to_string()];

        let response = service
            .dump_history(Request::new(request))
            .await
            .unwrap()
            .into_inner();

        assert!(!response.block.is_empty());
        for block in response.block {
            assert!(!block.native_bytes.is_empty());
            assert!(block.chain.is_none());
        }
    }

    #[tokio::test]
    async fn test_dump_history_max_items() {
        let domain = ToyDomain::new(None, None);
        let cancel = CancelTokenImpl::default();

        let service = SyncServiceImpl::new(domain, cancel);

        let request = u5c::sync::DumpHistoryRequest {
            start_token: None,
            max_items: MAX_DUMP_HISTORY_ITEMS + 1,
            field_mask: None,
        };

        let response = service
            .dump_history(Request::new(request))
            .await
            .unwrap_err();

        assert_eq!(response.code(), tonic::Code::InvalidArgument);
    }
}
