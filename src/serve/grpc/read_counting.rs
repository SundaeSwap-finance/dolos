use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::ThreadId;

use dolos_testing::toy_domain::ToyDomain;
use pallas::interop::utxorpc::{LedgerContext, TxoRef, UtxoMap};

use crate::prelude::*;

/// A domain that counts its store reads by whether they ran on the thread that
/// built it.
#[derive(Clone)]
pub struct ReadCountingDomain {
    inner: ToyDomain,
    runtime_thread: ThreadId,
    runtime_reads: Arc<AtomicUsize>,
    other_reads: Arc<AtomicUsize>,
}

impl ReadCountingDomain {
    pub fn new(inner: ToyDomain) -> Self {
        Self {
            inner,
            runtime_thread: std::thread::current().id(),
            runtime_reads: Default::default(),
            other_reads: Default::default(),
        }
    }

    /// Returns the reads on the building thread and whether any read ran on
    /// another thread, and resets both counts.
    pub fn take(&self) -> (usize, bool) {
        (
            self.runtime_reads.swap(0, Ordering::SeqCst),
            self.other_reads.swap(0, Ordering::SeqCst) > 0,
        )
    }

    fn count(&self) {
        if std::thread::current().id() == self.runtime_thread {
            self.runtime_reads.fetch_add(1, Ordering::SeqCst);
        } else {
            self.other_reads.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Domain for ReadCountingDomain {
    type Entity = <ToyDomain as Domain>::Entity;
    type EntityDelta = <ToyDomain as Domain>::EntityDelta;
    type Chain = <ToyDomain as Domain>::Chain;
    type WorkUnit = <ToyDomain as Domain>::WorkUnit;
    type Wal = <ToyDomain as Domain>::Wal;
    type State = <ToyDomain as Domain>::State;
    type Archive = <ToyDomain as Domain>::Archive;
    type Mempool = <ToyDomain as Domain>::Mempool;
    type TipSubscription = <ToyDomain as Domain>::TipSubscription;

    fn storage_config(&self) -> &dolos_core::config::StorageConfig {
        self.inner.storage_config()
    }

    fn sync_config(&self) -> &dolos_core::config::SyncConfig {
        self.inner.sync_config()
    }

    fn genesis(&self) -> Arc<Genesis> {
        self.inner.genesis()
    }

    fn read_chain(&self) -> std::sync::RwLockReadGuard<'_, Self::Chain> {
        self.count();
        self.inner.read_chain()
    }

    fn write_chain(&self) -> std::sync::RwLockWriteGuard<'_, Self::Chain> {
        self.count();
        self.inner.write_chain()
    }

    fn wal(&self) -> &Self::Wal {
        self.count();
        self.inner.wal()
    }

    fn state(&self) -> &Self::State {
        self.count();
        self.inner.state()
    }

    fn archive(&self) -> &Self::Archive {
        self.count();
        self.inner.archive()
    }

    fn mempool(&self) -> &Self::Mempool {
        self.count();
        self.inner.mempool()
    }

    fn watch_tip(&self, from: Option<ChainPoint>) -> Result<Self::TipSubscription, DomainError> {
        self.count();
        self.inner.watch_tip(from)
    }

    fn notify_tip(&self, tip: TipEvent) {
        self.inner.notify_tip(tip)
    }
}

impl LedgerContext for ReadCountingDomain {
    fn get_utxos(&self, refs: &[TxoRef]) -> Option<UtxoMap> {
        self.count();
        self.inner.get_utxos(refs)
    }

    fn get_slot_timestamp(&self, slot: u64) -> Option<u64> {
        self.count();
        self.inner.get_slot_timestamp(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_read_on_the_runtime_thread_counts_as_a_runtime_read() {
        let domain = ReadCountingDomain::new(ToyDomain::new(None, None));

        let _ = domain.state();

        assert_eq!(domain.take(), (1, false));
    }

    #[tokio::test]
    async fn a_read_on_the_blocking_pool_counts_as_another_read() {
        let domain = ReadCountingDomain::new(ToyDomain::new(None, None));

        let moved = domain.clone();
        tokio::task::spawn_blocking(move || {
            let _ = moved.state();
        })
        .await
        .unwrap();

        assert_eq!(domain.take(), (0, true));
    }
}
