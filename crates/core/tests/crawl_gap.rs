//! What a crawl does when the archive runs out where the wal does not reach.

use dolos_core::crawl::ChainCrawler;
use dolos_core::{
    ArchiveStore as _, ArchiveWriter as _, ChainPoint, Domain as _, DomainError, SyncExt as _,
};
use dolos_testing::blocks::make_conway_block;
use dolos_testing::toy_domain::ToyDomain;

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

    let (mut crawler, intersected) = ChainCrawler::<ToyDomain>::start(&domain, &[start.clone()])
        .unwrap()
        .expect("the wal holds the start point");

    assert_eq!(intersected.slot(), start.slot());

    let mut seen = vec![];

    while let Some((point, _)) = crawler.next_block().unwrap() {
        seen.push(point.slot());
    }

    assert_eq!(seen, vec![2, 3, 4, 5]);
}

/// The must-fire case. The crawl intersects in the archive, drains it, and finds
/// no wal entry to continue from.
#[test]
fn a_crawl_past_the_end_of_the_archive_names_the_gap() {
    let (domain, point) = domain_with_a_block_only_in_the_archive();

    let result = ChainCrawler::<ToyDomain>::start(&domain, &[point.clone()]);

    let Err(error) = result else {
        panic!("the crawl started past the end of the archive");
    };

    assert!(
        matches!(&error, DomainError::ArchiveWalGap(p) if p.slot() == point.slot()),
        "{error}"
    );
}
