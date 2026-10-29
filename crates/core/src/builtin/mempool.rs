//! Built-in in-memory mempool implementation.
//!
//! A basic FIFO mempool backed by in-memory data structures and a
//! broadcast channel for event notifications. Suitable for single-node
//! deployments and development/testing.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{Arc, RwLock},
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::debug;

use crate::{
    ChainPoint, MempoolError, MempoolEvent, MempoolPage, MempoolStore, MempoolTx, MempoolTxStage,
    TxHash, TxStatus,
};

/// The position a transaction took in the pending queue when first admitted.
type Seq = u64;

#[derive(Default)]
struct MempoolState {
    pending: BTreeMap<(Seq, TxHash), MempoolTx>,
    inflight: Vec<(Seq, MempoolTx)>,
    acknowledged: HashMap<TxHash, (Seq, MempoolTx)>,
    finalized_log: VecDeque<MempoolTx>,
}

impl MempoolState {
    fn next_seq(&self) -> Seq {
        let pending = self.pending.keys().next_back().map(|(seq, _)| *seq);
        let inflight = self.inflight.iter().map(|(seq, _)| *seq);
        let acknowledged = self.acknowledged.values().map(|(seq, _)| *seq);
        pending
            .into_iter()
            .chain(inflight)
            .chain(acknowledged)
            .max()
            .map_or(0, |seq| seq + 1)
    }

    fn take_pending(&mut self, hashes: &HashSet<&TxHash>) -> Vec<(Seq, MempoolTx)> {
        let taken: Vec<(Seq, TxHash)> = self
            .pending
            .keys()
            .filter(|(_, hash)| hashes.contains(&hash))
            .copied()
            .collect();
        taken
            .into_iter()
            .filter_map(|key| self.pending.remove(&key).map(|tx| (key.0, tx)))
            .collect()
    }
}

const MAX_FINALIZED_LOG: usize = 1000;

/// A basic, FIFO, in-memory mempool.
#[derive(Clone)]
pub struct EphemeralMempool {
    state: Arc<RwLock<MempoolState>>,
    updates: broadcast::Sender<MempoolEvent>,
}

impl Default for EphemeralMempool {
    fn default() -> Self {
        Self::new()
    }
}

impl EphemeralMempool {
    pub fn new() -> Self {
        let state = Arc::new(RwLock::new(MempoolState::default()));
        let (updates, _) = broadcast::channel(16);

        Self { state, updates }
    }

    fn notify(&self, tx: MempoolTx) {
        if self.updates.send(MempoolEvent { tx }).is_err() {
            debug!("no mempool update receivers");
        }
    }

    fn log_state(&self, state: &MempoolState) {
        debug!(
            pending = state.pending.len(),
            inflight = state.inflight.len(),
            acknowledged = state.acknowledged.len(),
            "mempool state changed"
        );
    }
}

pub struct EphemeralMempoolStream {
    inner: BroadcastStream<MempoolEvent>,
}

impl futures_core::Stream for EphemeralMempoolStream {
    type Item = Result<MempoolEvent, MempoolError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use futures_util::StreamExt;

        match self.inner.poll_next_unpin(cx) {
            std::task::Poll::Ready(Some(x)) => match x {
                Ok(x) => std::task::Poll::Ready(Some(Ok(x))),
                Err(err) => {
                    std::task::Poll::Ready(Some(Err(MempoolError::Internal(Box::new(err)))))
                }
            },
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl MempoolStore for EphemeralMempool {
    type Stream = EphemeralMempoolStream;

    fn receive(&self, tx: MempoolTx) -> Result<(), MempoolError> {
        let mut state = self.state.write().unwrap();

        if state.pending.values().any(|p| p.hash == tx.hash) {
            return Err(MempoolError::DuplicateTx);
        }

        debug!(tx.hash = %tx.hash, "tx received");
        let seq = state.next_seq();
        state.pending.insert((seq, tx.hash), tx.clone());
        self.notify(tx);
        self.log_state(&state);

        Ok(())
    }

    fn has_pending(&self) -> bool {
        let state = self.state.read().unwrap();
        !state.pending.is_empty()
    }

    fn peek_pending(&self) -> Vec<MempoolTx> {
        let state = self.state.read().unwrap();
        state.pending.values().cloned().collect()
    }

    fn mark_inflight(&self, hashes: &[TxHash]) -> Result<(), MempoolError> {
        let hash_set: HashSet<_> = hashes.iter().collect();
        let mut state = self.state.write().unwrap();

        for (seq, mut tx) in state.take_pending(&hash_set) {
            debug!(tx.hash = %tx.hash, "tx inflight");
            tx.stage = MempoolTxStage::Propagated;
            state.inflight.push((seq, tx.clone()));
            self.notify(tx);
        }

        self.log_state(&state);
        Ok(())
    }

    fn mark_acknowledged(&self, hashes: &[TxHash]) -> Result<(), MempoolError> {
        let hash_set: HashSet<_> = hashes.iter().collect();
        let mut state = self.state.write().unwrap();

        let mut moved = Vec::new();
        state.inflight.retain(|(seq, tx)| {
            if hash_set.contains(&tx.hash) {
                moved.push((*seq, tx.clone()));
                false
            } else {
                true
            }
        });

        for (seq, mut tx) in moved {
            debug!(tx.hash = %tx.hash, "tx acknowledged");
            tx.stage = MempoolTxStage::Acknowledged;
            state.acknowledged.insert(tx.hash, (seq, tx.clone()));
            self.notify(tx);
        }

        // confirm() also moves still-propagated entries into this map. A
        // later peer acknowledgement must update them, but never regress an
        // already confirmed transaction or reset its expiry counters.
        for hash in hashes {
            if let Some((_, tx)) = state.acknowledged.get_mut(hash) {
                if tx.stage == MempoolTxStage::Propagated {
                    tx.stage = MempoolTxStage::Acknowledged;
                    self.notify(tx.clone());
                }
            }
        }

        self.log_state(&state);
        Ok(())
    }

    fn find_inflight(&self, tx_hash: &TxHash) -> Option<MempoolTx> {
        let state = self.state.read().unwrap();
        // Check propagated (inflight vec)
        if let Some((_, tx)) = state.inflight.iter().find(|(_, x)| x.hash.eq(tx_hash)) {
            return Some(tx.clone());
        }
        // Check acknowledged/confirmed
        state.acknowledged.get(tx_hash).map(|(_, tx)| tx.clone())
    }

    fn peek_inflight(&self) -> Vec<MempoolTx> {
        let state = self.state.read().unwrap();

        state
            .inflight
            .iter()
            .chain(state.acknowledged.values())
            .map(|(_, tx)| tx.clone())
            .collect()
    }

    fn confirm(
        &self,
        point: &ChainPoint,
        seen_txs: &[TxHash],
        unseen_txs: &[TxHash],
        finalize_threshold: u32,
        drop_threshold: u32,
    ) -> Result<(), MempoolError> {
        let mut state = self.state.write().unwrap();
        let seen_set: HashSet<&TxHash> = seen_txs.iter().collect();
        let unseen_set: HashSet<&TxHash> = unseen_txs.iter().collect();

        // Promote Propagated txs into the acknowledged map so the confirm
        // logic below applies uniformly to every inflight tx.
        let propagated: Vec<(Seq, MempoolTx)> = state.inflight.drain(..).collect();
        // A pending transaction in the block is confirmed like an inflight one.
        let included = state.take_pending(&seen_set);
        for (seq, tx) in propagated.into_iter().chain(included) {
            state.acknowledged.insert(tx.hash, (seq, tx));
        }

        if state.acknowledged.is_empty() {
            return Ok(());
        }

        let hashes: Vec<TxHash> = state.acknowledged.keys().copied().collect();

        for tx_hash in hashes {
            if seen_set.contains(&tx_hash) {
                let (_, tx) = state.acknowledged.get_mut(&tx_hash).unwrap();
                tx.confirm(point);
                // Check if finalizable
                if tx.confirmations >= finalize_threshold {
                    let mut finalized = tx.clone();
                    finalized.stage = MempoolTxStage::Finalized;
                    state.finalized_log.push_back(finalized.clone());
                    state.acknowledged.remove(&tx_hash);
                    debug!(tx.hash = %tx_hash, "tx finalized");
                    self.notify(finalized);
                } else {
                    self.notify(tx.clone());
                    debug!(tx.hash = %tx_hash, "tx confirmed");
                }
            } else if unseen_set.contains(&tx_hash) {
                let (seq, mut tx) = state.acknowledged.remove(&tx_hash).unwrap();

                let mut event_tx = tx.clone();
                event_tx.stage = MempoolTxStage::RolledBack;
                self.notify(event_tx);

                tx.retry();
                state.pending.insert((seq, tx_hash), tx);
                debug!(tx.hash = %tx_hash, "retry tx");
            } else {
                let (_, tx) = state.acknowledged.get_mut(&tx_hash).unwrap();
                if tx.stage == MempoolTxStage::Confirmed {
                    // Already confirmed on-chain; treat subsequent blocks as
                    // additional confirmations (increasing depth).
                    tx.confirm(point);
                    if tx.confirmations >= finalize_threshold {
                        let mut finalized = tx.clone();
                        finalized.stage = MempoolTxStage::Finalized;
                        state.finalized_log.push_back(finalized.clone());
                        state.acknowledged.remove(&tx_hash);
                        debug!(tx.hash = %tx_hash, "tx finalized");
                        self.notify(finalized);
                    } else {
                        self.notify(tx.clone());
                    }
                } else {
                    tx.mark_stale();
                    // Check if droppable
                    if tx.non_confirmations >= drop_threshold {
                        let mut dropped = tx.clone();
                        dropped.stage = MempoolTxStage::Dropped;
                        state.finalized_log.push_back(dropped.clone());
                        state.acknowledged.remove(&tx_hash);
                        debug!(tx.hash = %tx_hash, "tx dropped");
                        self.notify(dropped);
                    }
                }
            }
        }

        if state.finalized_log.len() > MAX_FINALIZED_LOG {
            let excess = state.finalized_log.len() - MAX_FINALIZED_LOG;
            state.finalized_log.drain(..excess);
        }

        Ok(())
    }

    fn drop_pending(&self, hashes: &[TxHash]) -> Result<(), MempoolError> {
        let hash_set: HashSet<_> = hashes.iter().collect();
        let mut state = self.state.write().unwrap();

        for (_, mut tx) in state.take_pending(&hash_set) {
            tx.stage = MempoolTxStage::Dropped;
            state.finalized_log.push_back(tx.clone());
            debug!(tx.hash = %tx.hash, "pending tx dropped");
            self.notify(tx);
        }

        if state.finalized_log.len() > MAX_FINALIZED_LOG {
            let excess = state.finalized_log.len() - MAX_FINALIZED_LOG;
            state.finalized_log.drain(..excess);
        }

        self.log_state(&state);

        Ok(())
    }

    fn check_status(&self, tx_hash: &TxHash) -> TxStatus {
        let state = self.state.read().unwrap();

        if let Some((_, tx)) = state.acknowledged.get(tx_hash) {
            TxStatus {
                stage: tx.stage.clone(),
                confirmations: tx.confirmations,
                non_confirmations: tx.non_confirmations,
                confirmed_at: tx.confirmed_at.clone(),
            }
        } else if let Some((_, tx)) = state.inflight.iter().find(|(_, x)| x.hash.eq(tx_hash)) {
            TxStatus {
                stage: tx.stage.clone(),
                confirmations: 0,
                non_confirmations: 0,
                confirmed_at: None,
            }
        } else if state.pending.values().any(|x| x.hash.eq(tx_hash)) {
            TxStatus {
                stage: MempoolTxStage::Pending,
                confirmations: 0,
                non_confirmations: 0,
                confirmed_at: None,
            }
        } else {
            TxStatus {
                stage: MempoolTxStage::Unknown,
                confirmations: 0,
                non_confirmations: 0,
                confirmed_at: None,
            }
        }
    }

    fn dump_finalized(&self, cursor: u64, limit: usize) -> MempoolPage {
        let state = self.state.read().unwrap();
        let start = cursor as usize;

        let items: Vec<MempoolTx> = state
            .finalized_log
            .iter()
            .skip(start)
            .take(limit)
            .cloned()
            .collect();

        let end = start + items.len();
        let next_cursor = if end < state.finalized_log.len() {
            Some(end as u64)
        } else {
            None
        };

        MempoolPage { items, next_cursor }
    }

    fn subscribe(&self) -> Self::Stream {
        EphemeralMempoolStream {
            inner: BroadcastStream::new(self.updates.subscribe()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EraCbor;

    fn tx(n: u8) -> MempoolTx {
        MempoolTx::new(TxHash::new([n; 32]), EraCbor(7, vec![0x80]), vec![])
    }

    fn pending(store: &EphemeralMempool) -> Vec<TxHash> {
        store.peek_pending().iter().map(|tx| tx.hash).collect()
    }

    #[test]
    fn a_pending_transaction_admitted_later_stays_behind_one_requeued() {
        let store = EphemeralMempool::new();
        let (a, t, u) = (tx(1).hash, tx(2).hash, tx(3).hash);
        store.receive(tx(1)).unwrap();
        store.mark_inflight(&[a]).unwrap();
        store.receive(tx(2)).unwrap();
        store
            .confirm(&ChainPoint::Slot(11), &[a], &[], 10, 10)
            .unwrap();
        store
            .confirm(&ChainPoint::Slot(7), &[], &[a], 10, 10)
            .unwrap();
        assert_eq!(pending(&store), [a, t]);
        store.receive(tx(3)).unwrap();
        assert_eq!(pending(&store), [a, t, u]);
    }

    #[test]
    fn a_pending_transaction_seen_in_a_block_is_confirmed() {
        let store = EphemeralMempool::new();
        let (a, t) = (tx(1).hash, tx(2).hash);
        store.receive(tx(1)).unwrap();
        store.receive(tx(2)).unwrap();
        store
            .confirm(&ChainPoint::Slot(12), &[a], &[], 10, 10)
            .unwrap();
        assert_eq!(store.check_status(&a).stage, MempoolTxStage::Confirmed);
        assert_eq!(
            store.check_status(&a).confirmed_at,
            Some(ChainPoint::Slot(12))
        );
        assert_eq!(pending(&store), [t]);
    }
}
