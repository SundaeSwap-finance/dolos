use std::{
    cell::RefCell,
    fmt,
    panic::{self, AssertUnwindSafe},
    sync::{Arc, Once, PoisonError},
};

use dolos_cardano::CardanoLogic;
use dolos_core::{
    BlockHash, BlockSlot, ChainError, ChainLogic as _, ChainPoint, Domain as _, DomainError,
    EraCbor, MempoolAwareUtxoStore, RawBlock, StateStore as _, StateWriter as _, SubmitExt as _,
    SyncExt as _, TxHash, TxoRef, UtxoSetDelta,
};
use ledger_oracle_judge::{
    record::{Phase2, UtxoEntry},
    verdict::DolosVerdict,
};
use pallas::{
    codec::minicbor,
    ledger::{
        primitives::{
            dijkstra::{
                Block, BlockBody, BlockTransaction, Header, HeaderBody, HeaderVersionInfo,
                MempoolTransaction, OperationalCert,
            },
            KeepRaw, MaybeIndefArray, Nullable, VrfCert,
        },
        traverse::MultiEraBlock,
        validate::utils::ValidationError,
    },
};

use super::seed::{txo_ref, DIJKSTRA};
use crate::adapters::{DomainAdapter, MempoolBackend};

/// The hard fork variant that tags a Dijkstra block.
const DIJKSTRA_BLOCK: u16 = 8;

/// Why Dolos did not apply a block.
#[derive(Debug)]
pub(super) enum RollFailure {
    Error(DomainError),
    Panic { at: PanicAt, message: String },
}

impl fmt::Display for RollFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RollFailure::Error(e) => write!(f, "error {e}"),
            RollFailure::Panic { at, message } => write!(f, "panic at={at} {message}"),
        }
    }
}

/// The source location of a panic, as the panic hook reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PanicAt {
    Source(String),
    Unreported,
}

impl fmt::Display for PanicAt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PanicAt::Source(location) => write!(f, "{location}"),
            PanicAt::Unreported => write!(f, "unreported"),
        }
    }
}

thread_local! {
    /// The location of the last panic on this thread.
    static LAST_PANIC: RefCell<PanicAt> = const { RefCell::new(PanicAt::Unreported) };
}

/// Installs, once, a panic hook that records each panic's location for its thread and then runs the hook it replaces.
fn record_panic_locations() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let at = match info.location() {
                Some(l) => PanicAt::Source(format!("{}:{}:{}", l.file(), l.line(), l.column())),
                None => PanicAt::Unreported,
            };
            LAST_PANIC.with(|last| *last.borrow_mut() = at);
            previous(info);
        }));
    });
}

/// The header hash and block number of the last block a run applied.
#[derive(Debug, Clone, Copy)]
pub(super) struct Tip {
    hash: BlockHash,
    number: u64,
}

impl Tip {
    /// The header hash of the block.
    pub(super) fn hash(&self) -> BlockHash {
        self.hash
    }
}

/// A block at the slot whose parent is the tip, holding each transaction with its scripts flag, and the tip it makes.
pub(super) fn block(slot: u64, tip: Option<Tip>, txs: &[(&[u8], Phase2)]) -> (Tip, RawBlock) {
    let transactions = txs
        .iter()
        .map(|(raw, phase2)| {
            let tx: MempoolTransaction =
                minicbor::decode(raw).expect("a corpus transaction decodes");
            let success = match phase2 {
                Phase2::Valid => true,
                Phase2::Invalid => false,
            };
            BlockTransaction {
                success,
                ..BlockTransaction::from(tx)
            }
        })
        .collect();
    let number = tip.map_or(1, |t| t.number + 1);
    let header = Header {
        header_body: HeaderBody {
            block_number: number,
            slot,
            prev_hash: tip.map(|t| t.hash),
            issuer_vkey: vec![0; 32].into(),
            vrf_vkey: vec![0; 32].into(),
            vrf_result: VrfCert(vec![0; 64].into(), vec![0; 80].into()),
            block_body_size: 0,
            block_body_hash: [0; 32].into(),
            operational_cert: OperationalCert {
                operational_cert_hot_vkey: vec![0; 32].into(),
                operational_cert_sequence_number: 0,
                operational_cert_kes_period: 0,
                operational_cert_sigma: vec![0; 64].into(),
            },
            header_version_info: HeaderVersionInfo {
                highest_supported_major_version: 12,
                self_reported_software_tag: 0,
            },
            block_body_contains_leios_cert: false,
            eb_announcement: Nullable::Null,
        },
        body_signature: vec![0; 448].into(),
    };
    let block = Block {
        header: KeepRaw::from(header),
        block_body: BlockBody {
            transactions: MaybeIndefArray::Def(transactions),
            leios_certificate: Nullable::Null,
            peras_certificate: Nullable::Null,
        },
    };
    let raw = minicbor::to_vec((DIJKSTRA_BLOCK, &block)).expect("a block encodes");
    let hash = MultiEraBlock::decode(&raw)
        .expect("a written block decodes")
        .hash();
    (Tip { hash, number }, Arc::new(raw))
}

/// A domain over the seeded stores with a mempool of its own and its cursor at one slot.
pub(super) struct Group {
    domain: DomainAdapter,
}

/// The group for the records at one slot, whose mempool starts empty and whose cursor keeps the hash of the last applied block.
pub(super) fn group(base: &DomainAdapter, slot: u64) -> Group {
    let hash = base
        .state()
        .read_cursor()
        .unwrap()
        .and_then(|point| point.hash())
        .unwrap_or_else(|| [0; 32].into());
    let writer = base.state().start_writer().unwrap();
    writer.set_cursor(ChainPoint::Specific(slot, hash)).unwrap();
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

    /// Applies one block through the live sync path, reopening the chain logic from the state after a panic.
    pub(super) fn roll(&self, block: RawBlock) -> Result<BlockSlot, RollFailure> {
        record_panic_locations();
        LAST_PANIC.with(|last| *last.borrow_mut() = PanicAt::Unreported);
        let rolled = panic::catch_unwind(AssertUnwindSafe(|| self.domain.roll_forward(block)));
        let payload = match rolled {
            Ok(result) => return result.map_err(RollFailure::Error),
            Err(payload) => payload,
        };
        let at = LAST_PANIC.with(|last| last.replace(PanicAt::Unreported));
        let logic = CardanoLogic::initialize::<DomainAdapter>(
            Default::default(),
            &self.domain.state,
            &self.domain.genesis,
        )
        .unwrap();
        *self
            .domain
            .chain
            .write()
            .unwrap_or_else(PoisonError::into_inner) = logic;
        self.domain.chain.clear_poison();
        let message = match payload.downcast::<String>() {
            Ok(message) => *message,
            Err(payload) => payload
                .downcast_ref::<&str>()
                .map_or("non text payload", |m| m)
                .to_string(),
        };
        Err(RollFailure::Panic { at, message })
    }

    /// The domain the group submits to and rolls blocks into.
    pub(super) fn domain(&self) -> &DomainAdapter {
        &self.domain
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
