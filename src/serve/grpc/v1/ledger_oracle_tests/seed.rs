use std::sync::Arc;

use dolos_cardano::{
    model::{EpochState, EpochValue, PParamValue},
    pallas_extras, CardanoDelta, ControlledAmountInc, SingletonEntity as _,
};
use dolos_core::{
    apply_delta_chunk, config::CardanoConfig, save_entities, ChainLogic as _, ChainPoint,
    Domain as _, EntityMap, EraCbor, Genesis, StateStore as _, StateWriter as _, TxoRef,
    UtxoSetDelta, WalStore as _,
};
use dolos_testing::toy_domain::ToyDomain;
use ledger_oracle_judge::{cbor::TxIn, record::UtxoEntry};
use pallas::ledger::{
    addresses::{Address, ShelleyDelegationPart},
    traverse::{Era, MultiEraOutput},
};

use crate::adapters::{
    ArchiveStoreBackend, DomainAdapter, MempoolBackend, StateStoreBackend, WalAdapter,
};

/// The era tag Dolos stores a Dijkstra output under.
pub(super) const DIJKSTRA: u16 = 8;

/// The devnet genesis with the corpus epoch length and protocol version 12.
pub(super) fn genesis(epoch_size: u64) -> Arc<Genesis> {
    let mut genesis = dolos_cardano::include::devnet::load()
        .with_dijkstra(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/crates/core/test_data/proto-devnet/dijkstra-genesis.json"
        ))
        .expect("the devnet Dijkstra genesis loads");
    genesis.shelley.epoch_length =
        Some(u32::try_from(epoch_size).expect("the corpus epoch size fits a genesis epoch length"));
    genesis.force_protocol = Some(12);
    Arc::new(genesis)
}

/// The Dolos output reference for a ledger input.
pub(super) fn txo_ref(input: &TxIn) -> TxoRef {
    let index = u32::try_from(input.ix).expect("a ledger output index fits an output reference");
    TxoRef(input.tx.into(), index)
}

/// The account deltas Dolos makes when a block produces the outputs, leaving out outputs at pointer addresses.
fn controlled_amounts(pre: &[UtxoEntry], epoch: u64) -> Vec<CardanoDelta> {
    pre.iter()
        .filter_map(|e| {
            let output = MultiEraOutput::decode(Era::Dijkstra, &e.output.0)
                .expect("a seeded output decodes");
            let address = output.address().expect("a seeded output has an address");
            if let Address::Shelley(shelley) = &address {
                if let ShelleyDelegationPart::Pointer(_) = shelley.delegation() {
                    return None;
                }
            }
            let (cred, is_pointer) = pallas_extras::address_as_stake_cred(&address)?;
            let amount = output.value().coin();
            Some(ControlledAmountInc::new(cred, is_pointer, amount, epoch).into())
        })
        .collect()
}

/// A chain point the write ahead log starts at, made only by seeding a run.
pub(super) struct SeedPoint(ChainPoint);

impl SeedPoint {
    /// The chain point a rollback sweep returns to.
    pub(super) fn point(&self) -> &ChainPoint {
        &self.0
    }
}

/// A domain over memory stores that holds one run's first pre state.
pub(super) struct Seeded {
    pub(super) domain: DomainAdapter,
    pub(super) seed: SeedPoint,
    _toy: ToyDomain,
}

/// Seeds a domain with the given parameters and outputs at the first slot of the epoch.
pub(super) fn seed(
    genesis: &Arc<Genesis>,
    epoch: u64,
    pparams: &[PParamValue],
    pre: &[UtxoEntry],
) -> Seeded {
    let delta = UtxoSetDelta {
        produced_utxo: pre
            .iter()
            .map(|e| {
                let output = Arc::new(EraCbor(DIJKSTRA, e.output.0.clone()));
                (txo_ref(&e.input), output)
            })
            .collect(),
        ..Default::default()
    };
    let toy = ToyDomain::new_with_genesis_and_config(
        genesis.clone(),
        CardanoConfig::default(),
        Some(delta),
        None,
    );
    let slot = epoch * u64::from(genesis.shelley.epoch_length.unwrap_or_default());
    let point = ChainPoint::Specific(slot, [0; 32].into());
    let mut state = dolos_cardano::load_epoch::<ToyDomain>(toy.state()).unwrap();
    state.number = epoch;
    let live = state.pparams.unwrap_live_mut();
    for value in pparams {
        live.set(value.clone());
    }
    state.pparams = EpochValue::with_live(epoch, live.clone());
    let writer = toy.state().start_writer().unwrap();
    writer
        .write_entity_typed(&EpochState::singleton_key(), &state)
        .unwrap();
    writer.set_cursor(point.clone()).unwrap();
    writer.commit().unwrap();

    let state = StateStoreBackend::Memory(toy.state().clone());
    let chain = dolos_cardano::CardanoLogic::initialize::<DomainAdapter>(
        Default::default(),
        &state,
        genesis,
    )
    .unwrap();
    let domain = DomainAdapter {
        storage_config: Arc::new(toy.storage_config().clone()),
        sync_config: Arc::new(toy.sync_config().clone()),
        genesis: genesis.clone(),
        chain: Arc::new(std::sync::RwLock::new(chain)),
        state,
        archive: ArchiveStoreBackend::Memory(toy.archive().clone()),
        wal: WalAdapter::Redb(toy.wal().clone()),
        mempool: MempoolBackend::Ephemeral(Default::default()),
        tip_broadcast: tokio::sync::broadcast::channel(8).0,
    };
    let mut deltas = controlled_amounts(pre, epoch);
    let mut entities = EntityMap::default();
    apply_delta_chunk::<DomainAdapter>(&mut entities, domain.state(), &mut deltas).unwrap();
    let writer = domain.state().start_writer().unwrap();
    save_entities::<DomainAdapter>(&writer, &entities).unwrap();
    writer.commit().unwrap();
    domain.wal().reset_to(&point).unwrap();
    Seeded {
        domain,
        seed: SeedPoint(point),
        _toy: toy,
    }
}
