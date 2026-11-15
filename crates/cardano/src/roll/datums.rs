//! Datum state tracking via entity-delta pattern.
//!
//! This module implements witness datum reference counting using the entity
//! system. Datums are stored with a reference count that tracks how many UTxOs
//! reference them. When a UTxO with a datum hash is produced, the refcount is
//! incremented. When consumed, it's decremented. When refcount reaches zero,
//! the datum is removed.
//!
//! Only witness datums (those in the transaction witness set with
//! `DatumOption::Hash`) are tracked. Inline datums (`DatumOption::Data`) are
//! not reference counted.

use std::collections::HashMap;

use dolos_core::{ChainError, Genesis, TxoRef};
use pallas::{
    crypto::hash::Hash,
    ledger::{
        primitives::{conway::DatumOption, Epoch},
        traverse::{MultiEraBlock, MultiEraInput, MultiEraOutput, MultiEraTx, OriginalHash},
    },
};

use super::{BlockVisitor, WorkDeltas};
use crate::owned::OwnedMultiEraOutput;
use crate::{DatumRefDecrement, DatumRefIncrement, PParamsSet};

/// Visitor that tracks witness datums and emits refcount deltas.
///
/// For each transaction:
/// 1. Collects witness datums from `tx.plutus_data()` into a temporary map
/// 2. When processing outputs with `DatumOption::Hash`, emits
///    `DatumRefIncrement`
/// 3. When processing inputs with `DatumOption::Hash`, emits
///    `DatumRefDecrement`
#[derive(Default)]
pub struct DatumVisitor {
    /// Witness datums collected from current transaction's plutus_data()
    witness_datums: HashMap<Hash<32>, Vec<u8>>,

    /// The witness datums of each top level transaction of the block, by the
    /// hash of each sub transaction it lists.
    top_witness_datums: HashMap<Hash<32>, HashMap<Hash<32>, Vec<u8>>>,
}

/// The witness datums of a transaction, by hash.
fn witness_datums(tx: &MultiEraTx) -> HashMap<Hash<32>, Vec<u8>> {
    tx.plutus_data()
        .iter()
        .map(|datum| (datum.original_hash(), datum.raw_cbor().to_vec()))
        .collect()
}

impl BlockVisitor for DatumVisitor {
    fn visit_root(
        &mut self,
        _deltas: &mut WorkDeltas,
        block: &MultiEraBlock,
        _genesis: &Genesis,
        _pparams: &PParamsSet,
        _epoch: Epoch,
        _epoch_start: u64,
        _protocol: u16,
    ) -> Result<(), ChainError> {
        self.top_witness_datums.clear();

        for tx in block.txs() {
            let subs = tx.sub_transactions();

            if subs.is_empty() {
                continue;
            }

            let datums = witness_datums(&tx);

            for sub in subs {
                self.top_witness_datums.insert(sub.hash(), datums.clone());
            }
        }

        Ok(())
    }

    fn visit_tx(
        &mut self,
        _deltas: &mut WorkDeltas,
        _block: &MultiEraBlock,
        tx: &MultiEraTx,
        _utxos: &HashMap<TxoRef, OwnedMultiEraOutput>,
    ) -> Result<(), ChainError> {
        self.witness_datums = witness_datums(tx);

        // The ledger accepts the datum for a sub transaction's output hash from
        // the top level witnesses too.
        if let Some(top) = self.top_witness_datums.get(&tx.hash()) {
            for (hash, bytes) in top {
                self.witness_datums
                    .entry(*hash)
                    .or_insert_with(|| bytes.clone());
            }
        }

        Ok(())
    }

    fn visit_output(
        &mut self,
        deltas: &mut WorkDeltas,
        _block: &MultiEraBlock,
        _tx: &MultiEraTx,
        _index: u32,
        output: &MultiEraOutput,
    ) -> Result<(), ChainError> {
        // Check if this output references a datum by hash
        if let Some(DatumOption::Hash(datum_hash)) = output.datum() {
            // Only emit increment if the datum is in the witness set
            if let Some(datum_bytes) = self.witness_datums.get(&datum_hash) {
                deltas.add_for_entity(DatumRefIncrement::new(datum_hash, datum_bytes.clone()));
            }
        }

        Ok(())
    }

    fn visit_input(
        &mut self,
        deltas: &mut WorkDeltas,
        _block: &MultiEraBlock,
        _tx: &MultiEraTx,
        _input: &MultiEraInput,
        resolved: &MultiEraOutput,
    ) -> Result<(), ChainError> {
        // Check if the consumed UTxO had a datum hash
        if let Some(DatumOption::Hash(datum_hash)) = resolved.datum() {
            deltas.add_for_entity(DatumRefDecrement::new(datum_hash));
        }

        Ok(())
    }
}
