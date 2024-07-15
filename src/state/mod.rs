use itertools::Itertools as _;
use pallas::{
    interop::utxorpc as interop,
    ledger::{
        configs::{byron, shelley},
        traverse::{MultiEraBlock, MultiEraTx},
    },
};
use std::collections::{HashMap, HashSet};

use crate::ledger::*;

pub mod redb;

/// A persistent store for ledger state
#[derive(Clone)]
pub enum LedgerStore {
    Redb(redb::LedgerStore),
}

impl LedgerStore {
    pub fn cursor(&self) -> Result<Option<ChainPoint>, LedgerError> {
        match self {
            LedgerStore::Redb(x) => x.cursor(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            LedgerStore::Redb(x) => x.is_empty(),
        }
    }

    pub fn get_pparams(&self, until: BlockSlot) -> Result<Vec<PParamsBody>, LedgerError> {
        match self {
            LedgerStore::Redb(x) => x.get_pparams(until),
        }
    }

    pub fn get_utxos(&self, refs: Vec<TxoRef>) -> Result<UtxoMap, LedgerError> {
        match self {
            LedgerStore::Redb(x) => x.get_utxos(refs),
        }
    }

    pub fn get_utxo_by_address_set(&self, address: &[u8]) -> Result<HashSet<TxoRef>, LedgerError> {
        match self {
            LedgerStore::Redb(x) => x.get_utxo_by_address_set(address),
        }
    }

    pub fn apply(&mut self, deltas: &[LedgerDelta]) -> Result<(), LedgerError> {
        match self {
            LedgerStore::Redb(x) => x.apply(deltas),
        }
    }

    pub fn finalize(&mut self, until: BlockSlot) -> Result<(), LedgerError> {
        match self {
            LedgerStore::Redb(x) => x.finalize(until),
        }
    }
}

impl From<redb::LedgerStore> for LedgerStore {
    fn from(value: redb::LedgerStore) -> Self {
        Self::Redb(value)
    }
}

impl interop::LedgerContext for LedgerStore {
    fn get_utxos<'a>(&self, refs: &[interop::TxoRef]) -> Option<interop::UtxoMap> {
        let refs: Vec<_> = refs.iter().map(|x| TxoRef::from(*x)).collect();

        let some = self
            .get_utxos(refs)
            .ok()?
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();

        Some(some)
    }
}

pub fn load_slice_for_block(
    block: &MultiEraBlock,
    store: &LedgerStore,
    unapplied_deltas: &[LedgerDelta],
) -> Result<LedgerSlice, LedgerError> {
    let txs: HashMap<_, _> = block.txs().into_iter().map(|tx| (tx.hash(), tx)).collect();

    // TODO: turn this into "referenced utxos" intead of just consumed.
    let consumed: HashSet<_> = txs
        .values()
        .flat_map(MultiEraTx::consumes)
        .map(|utxo| TxoRef(*utxo.hash(), utxo.index() as u32))
        .collect();

    let consumed_same_block: HashMap<_, _> = txs
        .iter()
        .flat_map(|(tx_hash, tx)| {
            tx.produces()
                .into_iter()
                .map(|(idx, utxo)| (TxoRef(*tx_hash, idx as u32), utxo.into()))
        })
        .filter(|(x, _)| consumed.contains(x))
        .collect();

    let consumed_unapplied_deltas: HashMap<_, _> = unapplied_deltas
        .iter()
        .flat_map(|d| d.produced_utxo.iter().chain(d.recovered_stxi.iter()))
        .filter(|(x, _)| consumed.contains(x))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let to_fetch = consumed
        .into_iter()
        .filter(|x| !consumed_same_block.contains_key(x))
        .filter(|x| !consumed_unapplied_deltas.contains_key(x))
        .collect_vec();

    let mut resolved_inputs = store.get_utxos(to_fetch)?;
    resolved_inputs.extend(consumed_same_block);
    resolved_inputs.extend(consumed_unapplied_deltas);

    // TODO: include reference scripts and collateral

    Ok(LedgerSlice { resolved_inputs })
}

pub fn import_block_batch(
    blocks: &[MultiEraBlock],
    store: &mut LedgerStore,
    byron: &byron::GenesisFile,
    shelley: &shelley::GenesisFile,
) -> Result<(), LedgerError> {
    let mut deltas: Vec<LedgerDelta> = vec![];

    for block in blocks {
        let context = load_slice_for_block(block, store, &deltas)?;
        let delta = compute_delta(block, context).map_err(LedgerError::BrokenInvariant)?;

        deltas.push(delta);
    }

    store.apply(&deltas)?;

    let tip = deltas
        .last()
        .and_then(|x| x.new_position.as_ref())
        .map(|x| x.0)
        .unwrap();

    let to_finalize = lastest_immutable_slot(tip, byron, shelley);
    store.finalize(to_finalize)?;

    Ok(())
}