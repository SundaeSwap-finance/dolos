use std::sync::Arc;

use dolos_core::{
    ChainError, ChainPoint, Domain as _, DomainError, EraCbor, MempoolAwareUtxoStore,
    StateStore as _, StateWriter as _, SubmitExt as _, TxHash, TxoRef, UtxoSetDelta,
};
use ledger_oracle_judge::{record::UtxoEntry, verdict::DolosVerdict};
use pallas::ledger::validate::utils::ValidationError;

use super::seed::{txo_ref, DIJKSTRA};
use crate::adapters::{DomainAdapter, MempoolBackend};

/// A domain over the seeded stores with a mempool of its own and its cursor at one slot.
pub(super) struct Group {
    domain: DomainAdapter,
}

/// The group for the records at one slot, whose mempool starts empty.
pub(super) fn group(base: &DomainAdapter, slot: u64) -> Group {
    let writer = base.state().start_writer().unwrap();
    writer
        .set_cursor(ChainPoint::Specific(slot, [0; 32].into()))
        .unwrap();
    writer.commit().unwrap();
    let mut domain = base.clone();
    domain.mempool = MempoolBackend::Ephemeral(Default::default());
    Group { domain }
}

impl Group {
    /// Submits one transaction to the group mempool.
    pub(super) fn receive(&self, tx: &[u8]) -> Result<TxHash, DomainError> {
        let chain = self.domain.read_chain();
        self.domain.receive_tx("ledger-oracle", &chain, tx)
    }

    /// Writes ledger outputs into the state the group reads.
    pub(super) fn seed(&self, entries: &[&UtxoEntry]) {
        let delta = UtxoSetDelta {
            produced_utxo: entries
                .iter()
                .map(|e| {
                    let output = Arc::new(EraCbor(DIJKSTRA, e.output.0.clone()));
                    (txo_ref(&e.input), output)
                })
                .collect(),
            ..Default::default()
        };
        let writer = self.domain.state().start_writer().unwrap();
        writer.apply_utxoset(&delta).unwrap();
        writer.commit().unwrap();
    }

    /// Whether the group reads an output at the reference, from the stores or its mempool.
    pub(super) fn sees(&self, txo: TxoRef) -> bool {
        let utxos =
            MempoolAwareUtxoStore::<DomainAdapter>::new(self.domain.state(), self.domain.mempool());
        !utxos.get_utxos([txo].into()).unwrap().is_empty()
    }
}

/// The verdict for one submission.
pub(super) fn verdict(result: &Result<TxHash, DomainError>) -> DolosVerdict {
    let refused = |kind: &str| DolosVerdict::Refused(kind.to_string());
    let e = match result {
        Ok(_) => return DolosVerdict::Accepted,
        Err(DomainError::ChainError(e)) => e,
        Err(DomainError::WalError(_)) => return refused("WalError"),
        Err(DomainError::StateError(_)) => return refused("StateError"),
        Err(DomainError::ArchiveError(_)) => return refused("ArchiveError"),
        Err(DomainError::MempoolError(_)) => return refused("MempoolError"),
        Err(DomainError::InconsistentState { .. }) => return refused("InconsistentState"),
        Err(DomainError::Internal(_)) => return refused("Internal"),
        Err(DomainError::WalIsEmpty) => return refused("WalIsEmpty"),
        Err(DomainError::RollbackTargetNotInWal(_)) => return refused("RollbackTargetNotInWal"),
        Err(DomainError::ArchiveWalGap(_)) => return refused("ArchiveWalGap"),
        Err(DomainError::StopEpochReached) => return refused("StopEpochReached"),
    };
    match e {
        ChainError::Phase1ValidationRejected(v) => validation_verdict(v),
        ChainError::Phase2ValidationRejected(_) => DolosVerdict::Phase2Rejected,
        ChainError::Phase2EvaluationError(_) => refused("Phase2EvaluationError"),
        ChainError::DijkstraWithdrawalConflict { .. } => refused("DijkstraWithdrawalConflict"),
        ChainError::CantReceiveBlock(_) => refused("CantReceiveBlock"),
        ChainError::BrokenInvariant(_) => refused("BrokenInvariant"),
        ChainError::DecodingError(_) => refused("DecodingError"),
        ChainError::CborDecodingError(_) => refused("CborDecodingError"),
        ChainError::InvalidNamespace(_) => refused("InvalidNamespace"),
        ChainError::AddressDecoding(_) => refused("AddressDecoding"),
        ChainError::StateError(_) => refused("StateError"),
        ChainError::ArchiveError(_) => refused("ArchiveError"),
        ChainError::GenesisFieldMissing(_) => refused("GenesisFieldMissing"),
        ChainError::InvalidConfig(_) => refused("InvalidConfig"),
        ChainError::PParamsNotFound(_) => refused("PParamsNotFound"),
        ChainError::NoActiveEpoch => refused("NoActiveEpoch"),
        ChainError::MissingGovState => refused("MissingGovState"),
        ChainError::EraNotFound => refused("EraNotFound"),
        ChainError::EpochValueVersionNotFound(_) => refused("EpochValueVersionNotFound"),
        ChainError::PoolSnapshotLagging { .. } => refused("PoolSnapshotLagging"),
        ChainError::MissingRewards => refused("MissingRewards"),
        ChainError::InvalidPoolParams => refused("InvalidPoolParams"),
        ChainError::InvalidProposalParams => refused("InvalidProposalParams"),
        ChainError::UnrecordableProposalPart(_) => refused("UnrecordableProposalPart"),
        ChainError::Consensus(_) => refused("Consensus"),
    }
}

/// The name of a Debug rendered variant, up to its first field.
fn variant_name(debug: String) -> String {
    let end = debug.find(['(', ' ', '{']).unwrap_or(debug.len());
    debug[..end].to_string()
}

fn validation_verdict(v: &ValidationError) -> DolosVerdict {
    match v {
        ValidationError::DijkstraUnsupported(label) => DolosVerdict::Unsupported(label.to_string()),
        ValidationError::PostAlonzo(e) => DolosVerdict::Refused(variant_name(format!("{e:?}"))),
        other => DolosVerdict::Refused(variant_name(format!("{other:?}"))),
    }
}
