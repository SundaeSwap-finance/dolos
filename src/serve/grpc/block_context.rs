//! Resolved inputs shared by all transaction mappings in one block response.
//! The map is discarded after the response; no state survives a rollback.
use pallas::{
    interop::utxorpc::{LedgerContext, TxoRef, UtxoMap},
    ledger::traverse::MultiEraBlock,
};
use std::{collections::HashSet, sync::Arc};

#[derive(Clone)]
pub(super) struct BlockContext<C: LedgerContext> {
    inner: C,
    resolved: Option<Arc<UtxoMap>>,
}

impl<C: LedgerContext> BlockContext<C> {
    pub(super) fn new(inner: C, block: &MultiEraBlock<'_>) -> Self {
        let start = std::time::Instant::now();
        let refs: HashSet<TxoRef> = block
            .txs()
            .iter()
            .flat_map(|tx| {
                tx.inputs()
                    .into_iter()
                    .chain(tx.collateral())
                    .chain(tx.reference_inputs())
                    .map(|i| (*i.hash(), i.index() as u32))
                    .collect::<Vec<_>>()
            })
            .collect();
        let refs: Vec<_> = refs.into_iter().collect();
        let resolved = if refs.is_empty() {
            Some(UtxoMap::new())
        } else {
            inner.get_utxos(&refs)
        };
        tracing::trace!(slot = block.slot(), input_count = refs.len(),
            resolved_count = resolved.as_ref().map(|x| x.len()),
            elapsed = ?start.elapsed(), "Prepared block input context");
        Self {
            inner,
            resolved: resolved.map(Arc::new),
        }
    }
}

impl<C: LedgerContext> LedgerContext for BlockContext<C> {
    fn get_utxos(&self, refs: &[TxoRef]) -> Option<UtxoMap> {
        match &self.resolved {
            Some(resolved) => Some(
                refs.iter()
                    .filter_map(|r| resolved.get(r).map(|v| (*r, v.clone())))
                    .collect(),
            ),
            // Preserve the old per-transaction behavior if the bulk read failed.
            None => self.inner.get_utxos(refs),
        }
    }
    fn get_slot_timestamp(&self, slot: u64) -> Option<u64> {
        self.inner.get_slot_timestamp(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dolos_testing::synthetic::{build_synthetic_blocks, SyntheticBlockConfig};
    use pallas::interop::utxorpc::{v1alpha, v1beta};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    #[derive(Clone)]
    struct CountingContext {
        calls: Arc<AtomicUsize>,
        outputs: Arc<Mutex<UtxoMap>>,
        fail_bulk: bool,
    }
    impl LedgerContext for CountingContext {
        fn get_utxos(&self, refs: &[TxoRef]) -> Option<UtxoMap> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_bulk && refs.len() > 2 {
                return None;
            }
            let outputs = self.outputs.lock().unwrap();
            Some(
                refs.iter()
                    .filter_map(|r| outputs.get(r).map(|v| (*r, v.clone())))
                    .collect(),
            )
        }
        fn get_slot_timestamp(&self, _: u64) -> Option<u64> {
            Some(1234)
        }
    }
    fn fixture() -> (Vec<dolos_core::RawBlock>, CountingContext) {
        let (blocks, _, _) = build_synthetic_blocks(SyntheticBlockConfig {
            block_count: 2,
            txs_per_block: 4,
            spend_previous_outputs: true,
            ..Default::default()
        });
        let parent = MultiEraBlock::decode(&blocks[0]).unwrap();
        let outputs = parent
            .txs()
            .iter()
            .flat_map(|tx| {
                tx.outputs()
                    .iter()
                    .enumerate()
                    .map(|(i, o)| ((tx.hash(), i as u32), (parent.era(), o.encode())))
                    .collect::<Vec<_>>()
            })
            .collect();
        (
            blocks,
            CountingContext {
                calls: Arc::default(),
                outputs: Arc::new(Mutex::new(outputs)),
                fail_bulk: false,
            },
        )
    }
    #[test]
    fn resolves_once_and_preserves_both_wire_versions() {
        let (blocks, context) = fixture();
        let block = MultiEraBlock::decode(&blocks[1]).unwrap();
        let alpha = v1alpha::Mapper::new(context.clone()).map_block(&block);
        let beta = v1beta::Mapper::new(context.clone()).map_block(&block);
        assert!(alpha
            .body
            .as_ref()
            .unwrap()
            .tx
            .iter()
            .any(|t| t.inputs.iter().any(|i| i.as_output.is_some())));
        context.calls.store(0, Ordering::Relaxed);
        let prepared = BlockContext::new(context.clone(), &block);
        assert_eq!(
            v1alpha::Mapper::new(prepared.clone()).map_block(&block),
            alpha
        );
        assert_eq!(v1beta::Mapper::new(prepared).map_block(&block), beta);
        assert_eq!(context.calls.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn new_response_does_not_reuse_previous_resolution() {
        let (blocks, context) = fixture();
        let block = MultiEraBlock::decode(&blocks[1]).unwrap();
        let before =
            v1alpha::Mapper::new(BlockContext::new(context.clone(), &block)).map_block(&block);
        context.outputs.lock().unwrap().clear();
        let after =
            v1alpha::Mapper::new(BlockContext::new(context.clone(), &block)).map_block(&block);
        assert_ne!(before, after);
        assert_eq!(after, v1alpha::Mapper::new(context).map_block(&block));
    }
    #[test]
    fn bulk_failure_falls_back_to_per_transaction_resolution() {
        let (blocks, mut context) = fixture();
        context.fail_bulk = true;
        let block = MultiEraBlock::decode(&blocks[1]).unwrap();
        let expected = v1alpha::Mapper::new(context.clone()).map_block(&block);
        let prepared = BlockContext::new(context.clone(), &block);
        assert!(prepared.resolved.is_none());
        assert_eq!(v1alpha::Mapper::new(prepared).map_block(&block), expected);
    }
}
