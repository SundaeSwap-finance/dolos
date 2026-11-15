//! What `SyncExt::rollback` does to a store the genesis has not been applied
//! to.

use std::sync::Arc;

use dolos_cardano::{CardanoLogic, EpochState};
use dolos_core::config::CardanoConfig;
use dolos_core::sync::SyncExt as _;
use dolos_core::{ChainLogic as _, ChainPoint, Domain as _, StateStore as _};
use dolos_testing::blocks::make_conway_block_with_prev;
use dolos_testing::toy_domain::ToyDomain;

fn domain() -> ToyDomain {
    let genesis = Arc::new(dolos_cardano::include::devnet::load());
    ToyDomain::before_genesis(genesis, CardanoConfig::default())
}

/// The chain logic a restarted process builds from the store.
fn restart(domain: &ToyDomain) {
    let fresh = CardanoLogic::initialize::<ToyDomain>(
        CardanoConfig::default(),
        domain.state(),
        &domain.genesis(),
    )
    .unwrap();

    *domain.write_chain() = fresh;
}

/// Rolls the first block forward and returns the cursor and the epoch state it
/// leaves.
fn first_block(domain: &ToyDomain) -> (ChainPoint, Option<ChainPoint>, EpochState) {
    let (point, block) = make_conway_block_with_prev(100, None, 1);
    domain.roll_forward(block).unwrap();

    let cursor = domain.state().read_cursor().unwrap();
    let epoch = dolos_cardano::load_epoch::<ToyDomain>(domain.state()).unwrap();

    (point, cursor, epoch)
}

/// The must-not case for the harness. A fresh store applies the genesis with
/// its first block when nothing rolls it back first.
#[test]
fn a_first_block_on_a_fresh_store_applies_the_genesis() {
    let domain = domain();

    let (point, cursor, epoch) = first_block(&domain);

    assert_eq!(cursor, Some(point));
    assert_eq!(epoch.number, 0);
}

/// The must-fire case a follower on a fresh store takes on every start, since
/// its first chain sync intersection is origin.
#[test]
fn a_rollback_to_origin_before_the_genesis_leaves_the_cursor_unset() {
    let domain = domain();

    domain.rollback(&ChainPoint::Origin).unwrap();

    assert_eq!(domain.state().read_cursor().unwrap(), None);
}

/// A process restarted after that rollback applies the genesis with its first
/// block.
#[test]
fn a_restart_after_a_rollback_to_origin_before_the_genesis_applies_the_genesis() {
    let domain = domain();
    domain.rollback(&ChainPoint::Origin).unwrap();

    restart(&domain);
    let (point, cursor, epoch) = first_block(&domain);

    assert_eq!(cursor, Some(point));
    assert_eq!(epoch.number, 0);
}

/// A follower that keeps running after that rollback applies the genesis with
/// its first block.
#[test]
fn a_live_follower_after_a_rollback_to_origin_before_the_genesis_applies_the_genesis() {
    let domain = domain();
    domain.rollback(&ChainPoint::Origin).unwrap();

    let (point, cursor, epoch) = first_block(&domain);

    assert_eq!(cursor, Some(point));
    assert_eq!(epoch.number, 0);
}
