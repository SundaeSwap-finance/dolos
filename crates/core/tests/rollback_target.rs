//! What `SyncExt::rollback` does with a target the wal does not hold.

use dolos_core::sync::SyncExt as _;
use dolos_core::{ChainPoint, Domain as _, DomainError, LogValue, StateStore as _, WalStore as _};
use dolos_testing::toy_domain::ToyDomain;

fn point(slot: u64) -> ChainPoint {
    ChainPoint::Specific(slot, [slot as u8; 32].into())
}

/// A wal holding slots 20 and 30 and nothing below them, which is what a pruned
/// front looks like from the rollback's side.
fn domain_with_a_pruned_front() -> ToyDomain {
    let domain = ToyDomain::new(None, None);

    let entries: Vec<_> = [20u64, 30]
        .into_iter()
        .map(|slot| (point(slot), LogValue::origin()))
        .collect();

    domain.wal().append_entries(&entries).unwrap();

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
