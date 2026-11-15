//! Before-images of the entities and logs that work outside a block writes,
//! kept so a rollback to the point the work was applied over restores them.

use std::collections::HashSet;
use std::ops::Range;

use pallas::codec::minicbor::{self, bytes::ByteVec, Decode, Encode};
use pallas::crypto::hash::Hasher;

use crate::{
    indexes::{ArchiveIndexDelta, IndexRecord, UtxoIndexDelta},
    ArchiveError, ArchiveStore, ArchiveWriter, BlockSlot, ChainPoint, Domain, DomainError, Entity,
    EntityKey, EntityMap, EntityValue, LogKey, Namespace, NsKey, RawBlock, StateError, StateStore,
    StateWriter, TemporalKey, UtxoSetDelta, WalStore,
};

/// The namespace that holds before-images, in the state store and in the
/// archive.
pub const UNDO_NS: Namespace = "undo";

const SLOT_SIZE: usize = 8;

/// Whether work applied over the store's cursor keeps the before-images a
/// rollback needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undo {
    /// The wal reaches the cursor, at this slot, so a rollback can return the
    /// store to it.
    Record(BlockSlot),

    /// The wal does not reach the cursor, so no rollback can return the store
    /// to it.
    Unreachable,
}

impl Undo {
    /// Decides for work about to be applied over the store's current cursor.
    pub fn for_work<D: Domain>(domain: &D) -> Result<Self, DomainError> {
        let Some(cursor) = domain.state().read_cursor()? else {
            return Ok(Self::Unreachable);
        };

        if domain.wal().reaches(&cursor)? {
            Ok(Self::Record(cursor.slot()))
        } else {
            Ok(Self::Unreachable)
        }
    }

    fn slot(&self) -> Option<BlockSlot> {
        match self {
            Self::Record(slot) => Some(*slot),
            Self::Unreachable => None,
        }
    }
}

/// The value a key held before the first write to it over a cursor.
#[derive(Debug, Encode, Decode)]
struct Record {
    #[n(0)]
    namespace: String,

    #[n(1)]
    key: ByteVec,

    #[n(2)]
    before: Option<ByteVec>,
}

impl Record {
    fn encode(namespace: Namespace, key: &[u8], before: Option<EntityValue>) -> EntityValue {
        let record = Record {
            namespace: namespace.to_string(),
            key: key.to_vec().into(),
            before: before.map(Into::into),
        };

        minicbor::to_vec(&record).unwrap()
    }
}

fn digest(namespace: Namespace, key: &[u8]) -> pallas::crypto::hash::Hash<32> {
    let mut hasher = Hasher::<256>::new();
    hasher.input(&[namespace.len() as u8]);
    hasher.input(namespace.as_bytes());
    hasher.input(key);
    hasher.finalize()
}

fn state_record_key(slot: BlockSlot, namespace: Namespace, key: &EntityKey) -> EntityKey {
    let digest = digest(namespace, key.as_ref());
    let bytes = [&slot.to_be_bytes()[..], &digest[..]].concat();
    EntityKey::from(bytes.as_slice())
}

fn archive_record_key(slot: BlockSlot, namespace: Namespace, key: &LogKey) -> LogKey {
    let digest = digest(namespace, key.as_ref());
    LogKey::from((TemporalKey::from(slot), EntityKey::from(&digest[..])))
}

fn record_slot(key: &[u8]) -> BlockSlot {
    let mut bytes = [0u8; SLOT_SIZE];
    bytes.copy_from_slice(&key[..SLOT_SIZE]);
    BlockSlot::from_be_bytes(bytes)
}

fn state_range(from: BlockSlot, until: Option<BlockSlot>) -> Range<EntityKey> {
    let end = match until {
        Some(slot) => EntityKey::from(&slot.to_be_bytes()[..]),
        None => EntityKey::full_range().end,
    };

    EntityKey::from(&from.to_be_bytes()[..])..end
}

fn archive_range(from: BlockSlot, until: Option<BlockSlot>) -> Range<LogKey> {
    let end = match until {
        Some(slot) => LogKey::from(TemporalKey::from(slot)),
        None => LogKey::full_range().end,
    };

    LogKey::from(TemporalKey::from(from))..end
}

fn namespace<E: Entity>(name: &str) -> Result<Namespace, DomainError> {
    E::NAMESPACES
        .iter()
        .copied()
        .find(|ns| *ns == name)
        .ok_or_else(|| {
            DomainError::Internal(format!(
                "a before-image names namespace {name}, which no entity has"
            ))
        })
}

/// A state writer that keeps the value each key held before the first write to
/// it over the cursor.
pub struct UndoStateWriter<S: StateStore> {
    store: S,
    inner: S::Writer,
    slot: Option<BlockSlot>,
}

impl<S: StateStore> UndoStateWriter<S> {
    pub fn start(store: &S, undo: Undo) -> Result<Self, StateError> {
        Ok(Self {
            store: store.clone(),
            inner: store.start_writer()?,
            slot: undo.slot(),
        })
    }

    fn keep(&self, namespace: Namespace, key: &EntityKey) -> Result<(), StateError> {
        let Some(slot) = self.slot else {
            return Ok(());
        };

        let record_key = state_record_key(slot, namespace, key);

        if self.store.read_entities(UNDO_NS, &[&record_key])?[0].is_some() {
            return Ok(());
        }

        let before = self.store.read_entities(namespace, &[key])?.remove(0);
        let record = Record::encode(namespace, key.as_ref(), before);

        self.inner.write_entity(UNDO_NS, &record_key, &record)
    }
}

impl<S: StateStore> StateWriter for UndoStateWriter<S> {
    fn set_cursor(&self, cursor: ChainPoint) -> Result<(), StateError> {
        self.inner.set_cursor(cursor)
    }

    fn write_entity(
        &self,
        ns: Namespace,
        key: &EntityKey,
        value: &EntityValue,
    ) -> Result<(), StateError> {
        self.keep(ns, key)?;
        self.inner.write_entity(ns, key, value)
    }

    fn delete_entity(&self, ns: Namespace, key: &EntityKey) -> Result<(), StateError> {
        self.keep(ns, key)?;
        self.inner.delete_entity(ns, key)
    }

    fn apply_utxoset(&self, delta: &UtxoSetDelta) -> Result<(), StateError> {
        self.inner.apply_utxoset(delta)
    }

    fn apply_utxo_tags(&self, delta: &UtxoIndexDelta) -> Result<(), StateError> {
        self.inner.apply_utxo_tags(delta)
    }

    fn undo_utxo_tags(&self, delta: &UtxoIndexDelta) -> Result<(), StateError> {
        self.inner.undo_utxo_tags(delta)
    }

    fn commit(self) -> Result<(), StateError> {
        self.inner.commit()
    }
}

/// An archive writer that keeps the value each log held before the first write
/// to it over the cursor.
pub struct UndoArchiveWriter<A: ArchiveStore> {
    store: A,
    inner: A::Writer,
    slot: Option<BlockSlot>,
}

impl<A: ArchiveStore> UndoArchiveWriter<A> {
    pub fn start(store: &A, undo: Undo) -> Result<Self, ArchiveError> {
        Ok(Self {
            store: store.clone(),
            inner: store.start_writer()?,
            slot: undo.slot(),
        })
    }

    fn keep(&self, namespace: Namespace, key: &LogKey) -> Result<(), ArchiveError> {
        let Some(slot) = self.slot else {
            return Ok(());
        };

        let record_key = archive_record_key(slot, namespace, key);

        if self.store.read_logs(UNDO_NS, &[&record_key])?[0].is_some() {
            return Ok(());
        }

        let before = self.store.read_logs(namespace, &[key])?.remove(0);
        let record = Record::encode(namespace, key.as_ref(), before);

        self.inner.write_log(UNDO_NS, &record_key, &record)
    }
}

impl<A: ArchiveStore> ArchiveWriter for UndoArchiveWriter<A> {
    fn apply(&self, point: &ChainPoint, block: &RawBlock) -> Result<(), ArchiveError> {
        self.inner.apply(point, block)
    }

    fn apply_index(&self, deltas: &[ArchiveIndexDelta]) -> Result<(), ArchiveError> {
        self.inner.apply_index(deltas)
    }

    fn undo_index(&self, deltas: &[ArchiveIndexDelta]) -> Result<(), ArchiveError> {
        self.inner.undo_index(deltas)
    }

    fn append_prehashed(
        &self,
        records: impl IntoIterator<Item = IndexRecord>,
    ) -> Result<(), ArchiveError> {
        self.inner.append_prehashed(records)
    }

    fn write_log(
        &self,
        ns: Namespace,
        key: &LogKey,
        value: &EntityValue,
    ) -> Result<(), ArchiveError> {
        self.keep(ns, key)?;
        self.inner.write_log(ns, key, value)
    }

    fn delete_log(&self, ns: Namespace, key: &LogKey) -> Result<(), ArchiveError> {
        self.keep(ns, key)?;
        self.inner.delete_log(ns, key)
    }

    fn undo(&self, point: &ChainPoint) -> Result<(), ArchiveError> {
        self.inner.undo(point)
    }

    fn commit(self) -> Result<(), ArchiveError> {
        self.inner.commit()
    }
}

/// The before-images a rollback to a point restores, oldest cursor first.
pub(crate) struct Journal {
    state: Vec<(EntityKey, Record)>,
    archive: Vec<(LogKey, Record)>,
    restored: Vec<EntityKey>,
}

impl Journal {
    /// Reads every before-image kept over a cursor at or after `to`.
    pub(crate) fn since<D: Domain>(domain: &D, to: &ChainPoint) -> Result<Self, DomainError> {
        let mut state = Vec::new();
        for entry in domain
            .state()
            .iter_entities(UNDO_NS, state_range(to.slot(), None))?
        {
            let (key, value) = entry?;
            state.push((key, minicbor::decode(&value).map_err(StateError::from)?));
        }

        let mut archive = Vec::new();
        for entry in domain
            .archive()
            .iter_logs(UNDO_NS, archive_range(to.slot(), None))?
        {
            let (key, value) = entry?;
            archive.push((key, minicbor::decode(&value).map_err(ArchiveError::from)?));
        }

        Ok(Self {
            state,
            archive,
            restored: Vec::new(),
        })
    }

    /// Puts into `entities` the before-image of each key written over a cursor
    /// at or after `slot`, or over any cursor when `slot` is `None`.
    pub(crate) fn restore_entities<E: Entity>(
        &mut self,
        slot: Option<BlockSlot>,
        entities: &mut EntityMap<E>,
    ) -> Result<(), DomainError> {
        while let Some((key, _)) = self.state.last() {
            if slot.is_some_and(|slot| record_slot(key.as_ref()) < slot) {
                break;
            }

            let (key, record) = self.state.pop().expect("last entry exists");

            let namespace = namespace::<E>(&record.namespace)?;

            let entity = record
                .before
                .map(|value| E::decode_entity(namespace, &value))
                .transpose()?;

            entities.insert(
                NsKey(namespace, EntityKey::from(record.key.as_slice())),
                entity,
            );

            self.restored.push(key);
        }

        Ok(())
    }

    /// Writes the before-image of each log and deletes every before-image the
    /// journal read.
    pub(crate) fn finish<E: Entity>(
        self,
        state: &impl StateWriter,
        archive: &impl ArchiveWriter,
    ) -> Result<(), DomainError> {
        for key in self
            .restored
            .iter()
            .chain(self.state.iter().map(|(key, _)| key))
        {
            state.delete_entity(UNDO_NS, key)?;
        }

        let mut written = HashSet::new();

        for (record_key, record) in self.archive {
            let namespace = namespace::<E>(&record.namespace)?;
            let key = LogKey::from(record.key.as_slice());

            if written.insert((namespace, key.clone())) {
                match record.before {
                    Some(value) => archive.write_log(namespace, &key, &value)?,
                    None => archive.delete_log(namespace, &key)?,
                }
            }

            archive.delete_log(UNDO_NS, &record_key)?;
        }

        Ok(())
    }
}

/// Deletes the before-images kept over a cursor the wal no longer reaches, at
/// most `max_slots` slots of them, and says whether none are left.
pub fn prune<D: Domain>(domain: &D, max_slots: u64) -> Result<bool, DomainError> {
    let floor = domain.wal().find_start()?.map(|(point, _)| point.slot());

    let first_state = domain
        .state()
        .iter_entities(UNDO_NS, state_range(0, floor))?
        .next()
        .transpose()?
        .map(|(key, _)| record_slot(key.as_ref()));

    let first_archive = domain
        .archive()
        .iter_logs(UNDO_NS, archive_range(0, floor))?
        .next()
        .transpose()?
        .map(|(key, _)| record_slot(key.as_ref()));

    let Some(first) = first_state.into_iter().chain(first_archive).min() else {
        return Ok(true);
    };

    let last = first.saturating_add(max_slots);
    let (until, done) = match floor {
        Some(floor) if floor <= last => (Some(floor), true),
        Some(_) => (Some(last), false),
        None => (None, true),
    };

    let state_writer = domain.state().start_writer()?;
    for entry in domain
        .state()
        .iter_entities(UNDO_NS, state_range(0, until))?
    {
        let (key, _) = entry?;
        state_writer.delete_entity(UNDO_NS, &key)?;
    }
    state_writer.commit()?;

    let archive_writer = domain.archive().start_writer()?;
    for entry in domain
        .archive()
        .iter_logs(UNDO_NS, archive_range(0, until))?
    {
        let (key, _) = entry?;
        archive_writer.delete_log(UNDO_NS, &key)?;
    }
    archive_writer.commit()?;

    Ok(done)
}
