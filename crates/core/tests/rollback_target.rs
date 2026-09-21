//! What `SyncExt::rollback` does with a target the wal does not hold.

use std::collections::BTreeSet;
use std::sync::Arc;

use dolos_core::sync::SyncExt as _;
use dolos_core::{
    ChainPoint, Domain as _, DomainError, LogValue, StateStore as _, TxoRef, WalStore,
};
use dolos_testing::synthetic::{build_synthetic_blocks, SyntheticBlockConfig};
use dolos_testing::toy_domain::ToyDomain;

fn point(slot: u64) -> ChainPoint {
    ChainPoint::Specific(slot, [slot as u8; 32].into())
}

/// A wal holding slots 20 and 30 and nothing below them, which is what a pruned
/// front looks like from the rollback's side. Pruning takes the origin entry
/// along with the rest of the history below the cutoff, so the wal is left
/// without one.
fn domain_with_a_pruned_front() -> ToyDomain {
    let domain = ToyDomain::new(None, None);

    let entries: Vec<_> = [20u64, 30]
        .into_iter()
        .map(|slot| (point(slot), LogValue::origin()))
        .collect();

    domain.wal().append_entries(&entries).unwrap();
    domain.wal().prune_history(15, None).unwrap();

    assert!(
        !domain.wal().contains_point(&ChainPoint::Origin).unwrap(),
        "the fixture is only a pruned front if the origin entry went with the prune",
    );

    domain
}

/// A wal with no entries at all, which is what a follower has before its first
/// block.
fn domain_with_an_empty_wal() -> ToyDomain {
    let domain = domain_with_a_pruned_front();

    WalStore::truncate_front(domain.wal(), &ChainPoint::Origin).unwrap();

    assert!(
        domain.wal().find_tip().unwrap().is_none(),
        "the fixture is only an empty wal if nothing survived the truncation",
    );

    domain
}

/// The must-not case. Rolling back to the tip is the shortest rollback that
/// still has to reach the cursor write, so it says the refusal the other two
/// arms exercise is not refusing everything.
#[test]
fn a_rollback_to_a_point_the_wal_holds_moves_the_cursor() {
    let domain = domain_with_a_pruned_front();

    domain.rollback(&point(30)).unwrap();

    assert_eq!(domain.state().read_cursor().unwrap(), Some(point(30)));
}

/// The must-not case a follower on a fresh store takes on every start. Origin
/// is the position before the first entry, so no entry stands for it, and a
/// rollback to it is still carried out and still writes the cursor.
#[test]
fn a_rollback_to_origin_on_an_empty_wal_moves_the_cursor_to_origin() {
    let domain = domain_with_an_empty_wal();

    domain.rollback(&ChainPoint::Origin).unwrap();

    assert_eq!(
        domain.state().read_cursor().unwrap(),
        Some(ChainPoint::Origin)
    );
}

/// Every utxo reference the state holds, as the set a rollback has to leave
/// behind.
fn utxo_refs(domain: &ToyDomain) -> BTreeSet<TxoRef> {
    domain
        .state()
        .iter_utxos()
        .unwrap()
        .map(|entry| entry.unwrap().0)
        .collect()
}

/// A domain that synced from genesis and pruned nothing, so its wal still
/// begins at the origin entry genesis wrote.
fn domain_synced_from_genesis() -> ToyDomain {
    let (blocks, _, config) = build_synthetic_blocks(SyntheticBlockConfig::default());
    let genesis = Arc::new(dolos_cardano::include::devnet::load());
    let domain = ToyDomain::new_with_genesis_and_config(genesis, config, None, None);

    for block in &blocks {
        domain.roll_forward(block.clone()).unwrap();
    }

    domain
}

/// The must-not case for origin with entries above it. Every one of them is
/// undone and the cursor lands on origin rather than staying at the tip the
/// rollback just discarded.
///
/// The state is compared against a domain that never applied a block, because
/// the counts hold either way: each synthetic block spends one input and
/// creates one output, so only the set says whether the undo reached them all.
#[test]
fn a_rollback_to_origin_on_an_unpruned_wal_undoes_every_entry_above_it() {
    let (_, _, config) = build_synthetic_blocks(SyntheticBlockConfig::default());
    let genesis = Arc::new(dolos_cardano::include::devnet::load());
    let fresh = ToyDomain::new_with_genesis_and_config(genesis, config, None, None);
    let at_origin = utxo_refs(&fresh);

    let domain = domain_synced_from_genesis();

    let (tip, _) = domain.wal().find_tip().unwrap().unwrap();

    assert!(domain.wal().contains_point(&ChainPoint::Origin).unwrap());
    assert_ne!(domain.state().read_cursor().unwrap(), None);

    domain.rollback(&ChainPoint::Origin).unwrap();

    assert_eq!(
        domain.state().read_cursor().unwrap(),
        Some(ChainPoint::Origin)
    );
    assert!(!domain.wal().contains_point(&tip).unwrap());
    assert_eq!(utxo_refs(&domain), at_origin);
}

/// The must-fire case for origin below a pruned front. The entries that wrote
/// the ledger below the front are gone, so the walk cannot undo them and the
/// cursor would name a position the state is nowhere near.
#[test]
fn a_rollback_to_origin_below_the_pruned_front_names_the_target_and_undoes_nothing() {
    let domain = domain_synced_from_genesis();

    let (tip, _) = domain.wal().find_tip().unwrap().unwrap();
    domain.wal().prune_history(0, None).unwrap();

    assert!(!domain.wal().contains_point(&ChainPoint::Origin).unwrap());
    assert!(domain.wal().contains_point(&tip).unwrap());

    let before_cursor = domain.state().read_cursor().unwrap();
    let before_refs = utxo_refs(&domain);

    let error = domain.rollback(&ChainPoint::Origin).unwrap_err();

    assert!(
        matches!(&error, DomainError::RollbackTargetNotInWal(p) if p == &ChainPoint::Origin),
        "{error}"
    );
    assert_eq!(domain.state().read_cursor().unwrap(), before_cursor);
    assert_eq!(utxo_refs(&domain), before_refs);
    assert!(domain.wal().contains_point(&tip).unwrap());
}

/// The must-fire case for a wal seeded at a point that is not origin, which is
/// what a store bootstrapped from a snapshot holds. Nothing below the seed was
/// ever written to the wal, so nothing below it can be undone.
#[test]
fn a_rollback_to_origin_on_a_wal_seeded_above_it_names_the_target() {
    let domain = domain_with_a_pruned_front();

    let before = domain.state().read_cursor().unwrap();
    let error = domain.rollback(&ChainPoint::Origin).unwrap_err();

    assert!(
        matches!(&error, DomainError::RollbackTargetNotInWal(p) if p == &ChainPoint::Origin),
        "{error}"
    );
    assert_eq!(domain.state().read_cursor().unwrap(), before);
    assert!(domain.wal().contains_point(&point(30)).unwrap());
}

/// The must-fire case for a target below the pruned front. Every entry the wal
/// holds is above it, so the walk undoes all of them and reaches no cursor
/// write.
#[test]
fn a_rollback_below_the_pruned_front_names_the_target_and_undoes_nothing() {
    let domain = domain_with_a_pruned_front();

    let before = domain.state().read_cursor().unwrap();
    let error = domain.rollback(&point(10)).unwrap_err();

    assert!(
        matches!(&error, DomainError::RollbackTargetNotInWal(p) if p == &point(10)),
        "{error}"
    );
    assert_eq!(domain.state().read_cursor().unwrap(), before);
    assert!(domain.wal().contains_point(&point(30)).unwrap());
}

/// The must-fire case for a target above the tip, where the walk visits no
/// entry at all and every step after it runs on an empty set.
#[test]
fn a_rollback_above_the_tip_names_the_target_and_undoes_nothing() {
    let domain = domain_with_a_pruned_front();

    let before = domain.state().read_cursor().unwrap();
    let error = domain.rollback(&point(40)).unwrap_err();

    assert!(
        matches!(&error, DomainError::RollbackTargetNotInWal(p) if p == &point(40)),
        "{error}"
    );
    assert_eq!(domain.state().read_cursor().unwrap(), before);
    assert!(domain.wal().contains_point(&point(30)).unwrap());
}
