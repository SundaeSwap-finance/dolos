//! What `SyncExt::rollback` leaves in the store when the blocks it undoes
//! crossed the reward update or an epoch boundary.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use dolos_cardano::{CardanoLogic, EpochState, FixedNamespace as _, SingletonEntity as _};
use dolos_core::config::CardanoConfig;
use dolos_core::import::ImportExt as _;
use dolos_core::sync::SyncExt as _;
use dolos_core::undo::UNDO_NS;
use dolos_core::{
    ArchiveStore as _, ChainLogic as _, ChainPoint, Domain as _, EntityKey, LogKey, RawBlock,
    StateStore as _, TxoRef,
};
use dolos_testing::blocks::make_conway_block_with_prev;
use dolos_testing::toy_domain::ToyDomain;
use pallas::codec::minicbor;
use pallas::crypto::hash::Hasher;
use pallas::interop::hardano::configs::shelley::{Credential, Pool, RewardAccount, Staking};
use pallas::ledger::primitives::alonzo::RationalNumber;

const VRF_KEY: &str = "d8252bd637a90ba4dbd2cf63afda20a19888b7895ede067081ce7fb7411a972b";
const REWARD_KEY: &str = "bc2b888f42c68e4e118a4fa16ada52db896bffb61ceea51a58301bf6";
const DELEGATOR: &str = "5e81366cb6f3c0d14837614afcea669d51b8be9519eaec4a237504f8";

/// The first slot of epoch one on the devnet genesis.
const EPOCH_ONE: u64 = 432_000;

/// A slot past the reward update trigger of epoch zero, which the devnet
/// genesis puts at slot 172800.
const AFTER_REWARD_UPDATE: u64 = 200_000;

/// Every table a block or an epoch transition writes, read back in full.
#[derive(Debug, PartialEq)]
struct Store {
    cursor: Option<ChainPoint>,
    entities: BTreeMap<(&'static str, EntityKey), Vec<u8>>,
    logs: BTreeMap<(&'static str, LogKey), Vec<u8>>,
    utxos: BTreeMap<TxoRef, Vec<u8>>,
}

impl Store {
    fn read(domain: &ToyDomain) -> Self {
        let namespaces: BTreeSet<&'static str> = dolos_cardano::model::build_schema()
            .keys()
            .copied()
            .collect();

        let mut entities = BTreeMap::new();
        let mut logs = BTreeMap::new();

        for ns in namespaces {
            for entry in domain
                .state()
                .iter_entities(ns, EntityKey::full_range())
                .unwrap()
            {
                let (key, value) = entry.unwrap();
                entities.insert((ns, key), value);
            }

            for entry in domain
                .archive()
                .iter_logs(ns, LogKey::full_range())
                .unwrap()
            {
                let (key, value) = entry.unwrap();
                logs.insert((ns, key), value);
            }
        }

        let utxos = domain
            .state()
            .iter_utxos()
            .unwrap()
            .map(|entry| {
                let (txo, cbor) = entry.unwrap();
                (txo, cbor.1)
            })
            .collect();

        Self {
            cursor: domain.state().read_cursor().unwrap(),
            entities,
            logs,
            utxos,
        }
    }

    /// The namespaces whose records differ between the two stores, with the
    /// number of keys that differ in each.
    fn differences(&self, other: &Self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();

        if self.cursor != other.cursor {
            out.insert("cursor".to_string(), 1);
        }

        let keys: BTreeSet<_> = self.entities.keys().chain(other.entities.keys()).collect();
        for key in keys {
            if self.entities.get(key) != other.entities.get(key) {
                *out.entry(format!("state {}", key.0)).or_default() += 1;
            }
        }

        let keys: BTreeSet<_> = self.logs.keys().chain(other.logs.keys()).collect();
        for key in keys {
            if self.logs.get(key) != other.logs.get(key) {
                *out.entry(format!("archive {}", key.0)).or_default() += 1;
            }
        }

        let keys: BTreeSet<_> = self.utxos.keys().chain(other.utxos.keys()).collect();
        for key in keys {
            if self.utxos.get(key) != other.utxos.get(key) {
                *out.entry("utxos".to_string()).or_default() += 1;
            }
        }

        out
    }

    /// The epoch state singleton.
    fn epoch(&self) -> EpochState {
        let value = &self.entities[&(EpochState::NS, EpochState::singleton_key())];
        minicbor::decode(value).unwrap()
    }

    /// The number of before-images in the state store and the archive.
    fn undo_records(&self) -> usize {
        let state = self.entities.keys().filter(|(ns, _)| *ns == UNDO_NS);
        let archive = self.logs.keys().filter(|(ns, _)| *ns == UNDO_NS);
        state.count() + archive.count()
    }
}

/// A chain of empty blocks at the given slots, numbered on from `number`.
fn empty_blocks(slots: &[u64], number: u64) -> Vec<(ChainPoint, RawBlock)> {
    let mut prev = None;

    slots
        .iter()
        .enumerate()
        .map(|(i, slot)| {
            let (point, block) = make_conway_block_with_prev(*slot, prev, number + i as u64);
            prev = point.hash();
            (point, block)
        })
        .collect()
}

/// A stake pool declared in the genesis, shaped as the Musashi genesis
/// declares one.
fn genesis_pool(operator: &str) -> Pool {
    Pool {
        cost: 0,
        margin: RationalNumber {
            numerator: 0,
            denominator: 1,
        },
        metadata: None,
        owners: vec![],
        pledge: 0,
        public_key: operator.to_string(),
        relays: vec![],
        reward_account: RewardAccount {
            credential: Credential::KeyHash(REWARD_KEY.to_string()),
            network: "Testnet".to_string(),
        },
        vrf: VRF_KEY.to_string(),
        registration_deposit: None,
    }
}

/// A domain on the devnet genesis with one stake pool, and the configuration a
/// restart reads back.
///
/// The pool's operator is the hash of the empty issuer key the block builder
/// writes, so every block counts toward the pool's minted blocks for the epoch
/// the block is in.
fn domain() -> (ToyDomain, CardanoConfig) {
    let operator = hex::encode(Hasher::<224>::hash(&[]));

    let mut genesis = dolos_cardano::include::devnet::load();
    genesis.shelley.staking = Some(Staking {
        pools: Some(HashMap::from([(operator.clone(), genesis_pool(&operator))])),
        stake: Some(HashMap::from([(DELEGATOR.to_string(), operator)])),
    });

    let config = CardanoConfig::default();
    let domain =
        ToyDomain::new_with_genesis_and_config(Arc::new(genesis), config.clone(), None, None);

    (domain, config)
}

/// The chain logic a restarted process builds from the store.
fn restart(domain: &ToyDomain, config: CardanoConfig) {
    let fresh =
        CardanoLogic::initialize::<ToyDomain>(config, domain.state(), &domain.genesis()).unwrap();

    *domain.write_chain() = fresh;
}

fn roll(domain: &ToyDomain, blocks: &[(ChainPoint, RawBlock)]) {
    for (_, block) in blocks {
        domain.roll_forward(block.clone()).unwrap();
    }
}

/// Rolls forward to `target`, then over `ahead`, and rolls back to `target`.
///
/// Returns the store at `target`, at the tip, and after the rollback.
fn there_and_back(
    domain: &ToyDomain,
    before: &[(ChainPoint, RawBlock)],
    ahead: &[(ChainPoint, RawBlock)],
) -> (Store, Store, Store) {
    roll(domain, before);
    let target = before.last().unwrap().0.clone();
    let at_target = Store::read(domain);

    roll(domain, ahead);
    let at_tip = Store::read(domain);

    domain.rollback(&target).unwrap();
    let after = Store::read(domain);

    (at_target, at_tip, after)
}

/// The must-not case. The blocks undone all sit in epoch one after its
/// boundary work ran, so the block deltas alone restore the store.
#[test]
fn a_rollback_inside_an_epoch_restores_every_table() {
    let (domain, _) = domain();
    let blocks = empty_blocks(
        &[100, EPOCH_ONE - 1, EPOCH_ONE, EPOCH_ONE + 1, EPOCH_ONE + 2],
        10,
    );
    let (before, ahead) = blocks.split_at(3);

    let (at_target, at_tip, after) = there_and_back(&domain, before, ahead);

    assert_ne!(at_target.differences(&at_tip), BTreeMap::new());
    assert_eq!(after.differences(&at_target), BTreeMap::new());
}

/// The must-not case for the journal. An import writes no wal, so no rollback
/// can return to a point it passed and the epoch work keeps no before-images.
#[test]
fn an_import_across_an_epoch_boundary_keeps_no_before_images() {
    let (domain, _) = domain();
    let blocks = empty_blocks(&[100, AFTER_REWARD_UPDATE, EPOCH_ONE, EPOCH_ONE + 1], 10);

    domain
        .import_blocks(blocks.into_iter().map(|(_, block)| block).collect())
        .unwrap();
    let imported = Store::read(&domain);

    assert_eq!(imported.epoch().number, 1);
    assert_eq!(imported.undo_records(), 0);
}

/// The must-fire case for the reward update alone. The undone blocks cross the
/// slot that triggers it and stop short of the epoch boundary.
#[test]
fn a_rollback_across_the_reward_update_restores_every_table() {
    let (domain, _) = domain();
    let blocks = empty_blocks(&[100, AFTER_REWARD_UPDATE, AFTER_REWARD_UPDATE + 1], 10);
    let (before, ahead) = blocks.split_at(1);

    let (at_target, at_tip, after) = there_and_back(&domain, before, ahead);

    assert!(at_target.epoch().incentives.is_none());
    assert!(at_tip.epoch().incentives.is_some());
    assert_eq!(after.differences(&at_target), BTreeMap::new());
}

/// The must-fire case for the epoch boundary. The undone blocks cross the
/// boundary from epoch zero into epoch one, so the close and open of the epoch
/// ran between the target and the tip.
#[test]
fn a_rollback_across_an_epoch_boundary_restores_every_table() {
    let (domain, _) = domain();
    let blocks = empty_blocks(&[100, EPOCH_ONE - 1, EPOCH_ONE, EPOCH_ONE + 1], 10);
    let (before, ahead) = blocks.split_at(2);

    let (at_target, at_tip, after) = there_and_back(&domain, before, ahead);

    let written = at_target.differences(&at_tip);
    assert!(written.contains_key("state epochs"), "{written:?}");
    assert!(written.contains_key("state pools"), "{written:?}");
    assert_eq!(after.differences(&at_target), BTreeMap::new());
}

/// A process restarted after the rollback, as one after `doctor rollback` is,
/// replays the undone blocks to the tip it held before.
#[test]
fn a_restarted_replay_after_a_rollback_across_an_epoch_boundary_reaches_the_same_tip() {
    let (domain, config) = domain();
    let blocks = empty_blocks(&[100, EPOCH_ONE - 1, EPOCH_ONE, EPOCH_ONE + 1], 10);
    let (before, ahead) = blocks.split_at(2);

    let (_, at_tip, _) = there_and_back(&domain, before, ahead);

    restart(&domain, config);
    roll(&domain, ahead);

    assert_eq!(Store::read(&domain).differences(&at_tip), BTreeMap::new());
}

/// The must-not case for a follower that keeps running. The undone blocks sit
/// in one epoch, so the replay runs no epoch work.
#[test]
fn a_live_replay_after_a_rollback_inside_an_epoch_reaches_the_same_tip() {
    let (domain, _) = domain();
    let blocks = empty_blocks(&[100, EPOCH_ONE, EPOCH_ONE + 1, EPOCH_ONE + 2], 10);
    let (before, ahead) = blocks.split_at(2);

    let (_, at_tip, _) = there_and_back(&domain, before, ahead);

    roll(&domain, ahead);

    assert_eq!(Store::read(&domain).differences(&at_tip), BTreeMap::new());
}

/// A follower that keeps running after the rollback, as a live one does when
/// its peer switches forks, replays the undone blocks to the tip it held
/// before.
#[test]
fn a_live_replay_after_a_rollback_across_an_epoch_boundary_reaches_the_same_tip() {
    let (domain, _) = domain();
    let blocks = empty_blocks(&[100, EPOCH_ONE - 1, EPOCH_ONE, EPOCH_ONE + 1], 10);
    let (before, ahead) = blocks.split_at(2);

    let (_, at_tip, _) = there_and_back(&domain, before, ahead);

    roll(&domain, ahead);

    assert_eq!(Store::read(&domain).differences(&at_tip), BTreeMap::new());
}
