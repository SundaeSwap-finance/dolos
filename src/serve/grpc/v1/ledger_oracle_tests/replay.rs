//! The epoch boundary replay judge: Dolos applies a stored stretch of the
//! Musashi chain one block at a time, and its state at the first block of each
//! epoch is compared with the ledger's.

use std::{
    collections::BTreeSet,
    io::Read as _,
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
    sync::Arc,
};

use dolos_cardano::model::PoolState;
use dolos_core::{
    config::{CardanoConfig, SyncConfig},
    ChainPoint, Domain as _, Genesis, StateStore as _, SyncExt as _,
};
use dolos_testing::toy_domain::ToyDomain;
use ledger_oracle_judge::{
    judgement::{self, RunVerdict},
    replay::{self, ChainState, Checkpoint, Held, Pool, Pots, Replay, ReplayTally, Stop, Stopped},
    state::Hex,
};
use pallas::{codec::minicbor::Decoder, ledger::traverse::MultiEraBlock};

use super::{dolos_accounts, ledger_state, validate::panic_message};

fn test_data(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test_data")
        .join(path)
}

/// The blocks of the fixture, each as the node's chunk stores it.
fn blocks() -> Vec<Vec<u8>> {
    let gz = std::fs::File::open(test_data("ledger-oracle/replay-blocks.cbor.gz")).unwrap();
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .read_to_end(&mut bytes)
        .unwrap();
    let mut decoder = Decoder::new(&bytes);
    let mut blocks = Vec::new();
    while decoder.position() < bytes.len() {
        let start = decoder.position();
        decoder.skip().unwrap();
        blocks.push(bytes[start..decoder.position()].to_vec());
    }
    blocks
}

/// The state the ledger held at the first block of each epoch of the fixture.
fn checkpoints() -> Vec<Checkpoint> {
    let text = std::fs::read_to_string(test_data("ledger-oracle/replay-checkpoints.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The disagreements the ledger and Dolos are known to have over the fixture.
pub(super) fn known() -> BTreeSet<String> {
    let text = std::fs::read_to_string(test_data("ledger-oracle/replay-known.txt")).unwrap();
    replay::parse_known("replay-known.txt", &text).unwrap_or_else(|e| panic!("{e}"))
}

/// An empty store with the genesis, network magic and protocol version of the
/// Musashi chain the fixture was taken from.
fn musashi_genesis_domain() -> ToyDomain {
    let genesis = test_data("musashi-ttl/genesis");
    let genesis = Genesis::from_file_paths(
        genesis.join("byron.json"),
        genesis.join("shelley.json"),
        genesis.join("alonzo.json"),
        genesis.join("conway.json"),
        Some(12),
    )
    .unwrap()
    .with_dijkstra(genesis.join("dijkstra.json"))
    .unwrap();
    let config = CardanoConfig {
        magic: Some(164),
        is_testnet: true,
        stop_epoch: None,
        custom_utxos: vec![],
    };
    let mut sync = SyncConfig::default();
    sync.leios_lenient_apply = true;
    ToyDomain::new_with_genesis_and_config(Arc::new(genesis), config, None, None)
        .with_sync_config(sync)
}

/// The state Dolos holds, in the fields the ledger's state also holds.
fn held(toy: &ToyDomain) -> Result<Held, String> {
    let domain = crate::tests::toy_adapter(toy);
    let epoch = dolos_cardano::load_epoch::<ToyDomain>(toy.state()).map_err(|e| e.to_string())?;
    let hash = match toy.state().read_cursor().map_err(|e| e.to_string())? {
        Some(ChainPoint::Specific(_, hash)) => Hex(*hash),
        other => return Err(format!("cursor={other:?}")),
    };
    // The pots an epoch starts with are the pots after its first block only
    // when that block carries no transaction.
    match epoch.rolling.live().map(|r| r.tx_count) {
        Some(0) => {}
        txs => return Err(format!("checkpoint block tx_count={txs:?}")),
    }
    let pots = Pots {
        reserves: epoch.initial_pots.reserves,
        treasury: epoch.initial_pots.treasury,
        deposited: epoch.initial_pots.obligations(),
        fees: epoch.initial_pots.fees,
    };
    let pools = ledger_state::read_all::<PoolState>(&domain)
        .into_iter()
        .filter(|pool| pool.snapshot.live().is_some_and(|s| !s.is_retired))
        .map(|pool| {
            let held = Pool {
                deposit: pool.deposit,
                retiring: pool.retiring_epoch,
            };
            (Hex(*pool.operator), held)
        })
        .collect();
    let mut unheld = Vec::new();
    let dreps = ledger_state::dreps(&domain, &mut unheld);
    if !unheld.is_empty() {
        return Err(format!("{unheld:?}"));
    }
    let state = ChainState {
        pots,
        accounts: dolos_accounts(&domain),
        pools,
        dreps,
    };
    Ok(Held {
        epoch: epoch.number,
        hash,
        state,
    })
}

/// Applies each block to an empty Musashi store through the live sync path,
/// reading the state at each checkpoint slot, until the last block or the
/// first one Dolos does not apply.
fn replay(blocks: &[Vec<u8>], checkpoints: &[Checkpoint]) -> Replay {
    let toy = musashi_genesis_domain();
    let mut replay = Replay::default();
    for raw in blocks {
        let slot = match MultiEraBlock::decode(raw) {
            Ok(block) => block.slot(),
            Err(e) => {
                let detail = format!("decode: {e}");
                replay.stop = Some(Stop {
                    slot: 0,
                    how: Stopped::Refused,
                    detail,
                });
                return replay;
            }
        };
        let rolled =
            panic::catch_unwind(AssertUnwindSafe(|| toy.roll_forward(Arc::new(raw.clone()))));
        let stopped = match rolled {
            Ok(Ok(_)) => None,
            Ok(Err(e)) => Some((Stopped::Refused, e.to_string())),
            Err(payload) => Some((Stopped::Panicked, panic_message(payload))),
        };
        if let Some((how, detail)) = stopped {
            replay.stop = Some(Stop { slot, how, detail });
            return replay;
        }
        if checkpoints.iter().any(|c| c.slot == slot) {
            match held(&toy) {
                Ok(held) => {
                    replay.held.insert(slot, held);
                }
                Err(detail) => {
                    let how = Stopped::Unrepresentable;
                    replay.stop = Some(Stop { slot, how, detail });
                    return replay;
                }
            }
        }
    }
    replay
}

/// Replays the fixture's blocks and compares the state at each checkpoint.
pub(super) fn judge() -> ReplayTally {
    let checkpoints = checkpoints();
    ReplayTally::judge(&checkpoints, &replay(&blocks(), &checkpoints))
}

#[test]
fn replayed_state_at_each_epoch_start_is_the_ledgers() {
    let tally = judge();
    println!("{}", tally.line());
    let mut out = judgement::Tally::default();
    tally.report(&known(), &mut out);
    println!("{out}");
    assert!(matches!(out.verdict(), RunVerdict::Pass { .. }));
}
