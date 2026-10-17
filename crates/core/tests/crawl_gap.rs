//! What a crawl does when the archive runs out where the wal does not reach.

use dolos_core::crawl::ChainCrawler;
use dolos_core::{
    ArchiveStore as _, ArchiveWriter as _, BlockSlot, ChainPoint, Domain as _, DomainError,
    SyncExt as _, WalStore as _,
};
use dolos_testing::blocks::{make_conway_block, make_conway_block_with_prev};
use dolos_testing::toy_domain::ToyDomain;

const SLOTS: [BlockSlot; 10] = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];

/// Cuts the archive after `archive_end` and the wal before `wal_start`.
fn split_stores(domain: &ToyDomain, archive_end: BlockSlot, wal_start: BlockSlot) {
    let tip = SLOTS[SLOTS.len() - 1];

    domain
        .archive()
        .truncate_front(&ChainPoint::Slot(archive_end))
        .unwrap();

    domain.wal().prune_history(tip - wal_start, None).unwrap();

    let (archive_tip, _) = domain.archive().get_tip().unwrap().unwrap();
    let (wal_front, _) = domain.wal().find_start().unwrap().unwrap();

    assert_eq!(archive_tip, archive_end);
    assert_eq!(wal_front.slot(), wal_start);
}

/// A domain that rolled forward the blocks at `SLOTS`, with its archive ending
/// at `archive_end` and its wal starting at `wal_start`.
fn domain_split_at(archive_end: BlockSlot, wal_start: BlockSlot) -> ToyDomain {
    let domain = ToyDomain::new(None, None);

    for slot in SLOTS {
        domain.roll_forward(make_conway_block(slot).1).unwrap();
    }

    split_stores(&domain, archive_end, wal_start);

    domain
}

/// The points a crawl from `start` yields, and the error it stopped on if any.
fn crawl_from(domain: &ToyDomain, start: ChainPoint) -> (Vec<ChainPoint>, Result<(), DomainError>) {
    let (mut crawler, intersected) =
        ChainCrawler::<ToyDomain>::start(domain, std::slice::from_ref(&start))
            .unwrap()
            .expect("the archive holds the start point");

    assert_eq!(intersected, start);

    let mut seen = vec![];

    loop {
        match crawler.next_block() {
            Ok(Some((point, _))) => seen.push(point),
            Ok(None) => return (seen, Ok(())),
            Err(error) => return (seen, Err(error)),
        }
    }
}

/// A block the archive holds and the wal never saw, which is what the gap looks
/// like from the crawl's side once a pruned wal front has passed the archive
/// tip.
fn domain_with_a_block_only_in_the_archive() -> (ToyDomain, ChainPoint) {
    let domain = ToyDomain::new(None, None);

    let (point, block) = make_conway_block(100);

    let writer = domain.archive().start_writer().unwrap();
    writer.apply(&point, &block).unwrap();
    writer.commit().unwrap();

    (domain, point)
}

/// The must-not case. The same crawl over blocks the wal does hold runs to the
/// tip and reports the end as the end, so the arm below says the gap is what is
/// being refused rather than any archive intersect.
#[test]
fn a_crawl_the_wal_reaches_back_to_runs_to_the_tip() {
    let domain = ToyDomain::new(None, None);

    for slot in 1..=5u64 {
        let (_, block) = make_conway_block(slot);
        domain.roll_forward(block).unwrap();
    }

    let start = make_conway_block(1).0;

    let (mut crawler, intersected) =
        ChainCrawler::<ToyDomain>::start(&domain, std::slice::from_ref(&start))
            .unwrap()
            .expect("the wal holds the start point");

    assert_eq!(intersected.slot(), start.slot());

    let mut seen = vec![];

    while let Some((point, _)) = crawler.next_block().unwrap() {
        seen.push(point.slot());
    }

    assert_eq!(seen, vec![2, 3, 4, 5]);
}

/// The must-fire case. The crawl intersects in the archive, drains it, and
/// finds no wal entry to continue from.
#[test]
fn a_crawl_past_the_end_of_the_archive_names_the_gap() {
    let (domain, point) = domain_with_a_block_only_in_the_archive();

    let result = ChainCrawler::<ToyDomain>::start(&domain, std::slice::from_ref(&point));

    let Err(error) = result else {
        panic!("the crawl started past the end of the archive");
    };

    assert!(
        matches!(&error, DomainError::ArchiveWalGap(p) if p.slot() == point.slot()),
        "{error}"
    );
}

/// The must-fire case for a wal that starts after the archive ends.
#[test]
fn a_crawl_past_an_archive_that_ends_before_the_wal_starts_names_the_gap() {
    let domain = domain_split_at(40, 70);

    let (seen, end) = crawl_from(&domain, make_conway_block(20).0);

    assert!(
        matches!(&end, Err(DomainError::ArchiveWalGap(p)) if p.slot() == 40),
        "{end:?}"
    );

    let slots: Vec<_> = seen.iter().map(ChainPoint::slot).collect();
    assert_eq!(slots, vec![30, 40]);
}

/// The must-fire case for a wal that holds a different block at the slot where
/// the archive ends.
#[test]
fn a_crawl_names_the_gap_when_the_wal_holds_another_block_at_the_archive_end() {
    let domain = ToyDomain::new(None, None);

    for slot in SLOTS {
        let (_, other) = make_conway_block_with_prev(slot, None, 1);
        domain.roll_forward(other).unwrap();
    }

    domain
        .archive()
        .truncate_front(&ChainPoint::Origin)
        .unwrap();

    let writer = domain.archive().start_writer().unwrap();
    for slot in SLOTS {
        let (point, block) = make_conway_block(slot);
        writer.apply(&point, &block).unwrap();
    }
    writer.commit().unwrap();

    split_stores(&domain, 70, 50);

    let (seen, end) = crawl_from(&domain, make_conway_block(20).0);

    assert!(
        matches!(&end, Err(DomainError::ArchiveWalGap(p)) if p.slot() == 70),
        "{end:?}"
    );

    let slots: Vec<_> = seen.iter().map(ChainPoint::slot).collect();
    assert_eq!(slots, vec![30, 40, 50, 60, 70]);
}

/// The must-not case. The crawl reads the archive to its end, finds its last
/// block in the wal, and continues from the wal to the tip.
#[test]
fn a_crawl_from_the_archive_continues_into_the_wal_where_they_overlap() {
    let domain = domain_split_at(70, 50);

    let (seen, end) = crawl_from(&domain, make_conway_block(20).0);

    if let Err(error) = end {
        panic!("{error}");
    }

    let expected: Vec<_> = SLOTS[2..]
        .iter()
        .map(|slot| make_conway_block(*slot).0)
        .collect();

    assert_eq!(seen, expected);
}
