use crate::prelude::*;
use dolos_core::crawl::ChainCrawler;
use futures_core::Stream;

pub struct ChainStream;

impl ChainStream {
    /// The events from `intersect` onward, or `None` when no candidate point is
    /// in local history.
    ///
    /// The item is a `Result` because the stream would otherwise end the same
    /// way on a catch up that failed and on a cancellation, and a client has
    /// nothing else to read that tells the two apart.
    pub async fn start<D: Domain, C: CancelToken>(
        domain: D,
        intersect: Vec<ChainPoint>,
        cancel: C,
    ) -> Result<Option<impl Stream<Item = Result<TipEvent, DomainError>> + 'static>, DomainError>
    {
        let started =
            super::on_blocking_pool(move || ChainCrawler::<D>::start(&domain, &intersect))
                .await
                .map_err(|e| DomainError::Internal(e.to_string()))??;

        let Some((mut crawler, intersected)) = started else {
            return Ok(None);
        };

        Ok(Some(async_stream::stream! {
            yield Ok(TipEvent::Mark(intersected.clone()));

            loop {
                let read = super::on_blocking_pool(move || {
                    let next = crawler.next_block();
                    (crawler, next)
                })
                .await;

                let next = match read {
                    Ok((returned, next)) => {
                        crawler = returned;
                        next
                    }
                    Err(error) => {
                        yield Err(DomainError::Internal(error.to_string()));
                        return;
                    }
                };

                match next {
                    Ok(Some((point, block))) => yield Ok(TipEvent::Apply(point, block)),
                    Ok(None) => break,
                    Err(error) => {
                        yield Err(error);
                        return;
                    }
                }
            }

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        break;
                    }
                    next = crawler.next_tip() => {
                        yield Ok(next);
                    }
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use dolos_testing::blocks::make_conway_block;
    use dolos_testing::toy_domain::ToyDomain;
    use futures_util::{pin_mut, StreamExt};
    use tokio::time::timeout;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::serve::CancelTokenImpl;

    #[tokio::test]
    async fn test_stream_waiting() {
        let domain = ToyDomain::new(None, None);

        for i in 0..=100 {
            let (_, block) = make_conway_block(i * 10);

            use dolos_core::SyncExt;
            domain.roll_forward(block).unwrap();
        }

        let domain2 = domain.clone();
        let background = tokio::spawn(async move {
            for i in 101..=200 {
                let (_, block) = make_conway_block(i * 10);

                use dolos_core::SyncExt;
                domain2.roll_forward(block).unwrap();

                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });

        let chain_point = make_conway_block(500).0;
        let s = ChainStream::start::<ToyDomain, CancelTokenImpl>(
            domain,
            vec![chain_point.clone()],
            CancelTokenImpl(CancellationToken::new()),
        )
        .await
        .unwrap()
        .expect("intersect point should be found");

        pin_mut!(s);

        let first = s.next().await.unwrap().unwrap();

        assert_eq!(first, TipEvent::Mark(chain_point));

        for i in 51..=200 {
            let evt = timeout(Duration::from_secs(5), s.next())
                .await
                .expect("took too long");
            let value = evt.unwrap().unwrap();

            match value {
                TipEvent::Apply(p, _) => {
                    assert_eq!(p.slot(), i * 10)
                }
                _ => panic!("unexpected log value variant"),
            }
        }

        background.abort();
    }

    /// The must-fire case for the `Result` item. A catch up that cannot load
    /// its next page has to say so, because the stream otherwise ends exactly
    /// as a cancelled one does.
    #[tokio::test]
    async fn a_catch_up_that_fails_says_so_instead_of_ending() {
        use dolos_core::{ArchiveStore as _, ArchiveWriter as _};

        let domain = ToyDomain::new(None, None);

        // Two blocks the archive holds and the wal never saw, so the crawl
        // starts in the archive and runs out of it with nowhere to continue.
        let writer = domain.archive().start_writer().unwrap();
        for slot in [100u64, 110] {
            let (point, block) = make_conway_block(slot);
            writer.apply(&point, &block).unwrap();
        }
        writer.commit().unwrap();

        let start = make_conway_block(100).0;

        let s = ChainStream::start::<ToyDomain, CancelTokenImpl>(
            domain,
            vec![start],
            CancelTokenImpl(CancellationToken::new()),
        )
        .await
        .unwrap()
        .expect("the archive holds the start point");

        pin_mut!(s);

        assert!(matches!(s.next().await, Some(Ok(TipEvent::Mark(_)))));
        assert!(matches!(s.next().await, Some(Ok(TipEvent::Apply(_, _)))));

        let last = s
            .next()
            .await
            .expect("the stream ended instead of reporting the failed page");

        assert!(
            matches!(&last, Err(DomainError::ArchiveWalGap(_))),
            "{last:?}"
        );
    }

    #[tokio::test]
    async fn the_start_and_catch_up_read_the_stores_on_the_blocking_pool() {
        use crate::serve::grpc::read_counting::ReadCountingDomain;

        let toy = ToyDomain::new(None, None);
        for i in 0..=30 {
            use dolos_core::SyncExt;
            toy.roll_forward(make_conway_block(i * 10).1).unwrap();
        }
        let domain = ReadCountingDomain::new(toy);

        let s = ChainStream::start::<ReadCountingDomain, CancelTokenImpl>(
            domain.clone(),
            vec![make_conway_block(0).0],
            CancelTokenImpl(CancellationToken::new()),
        )
        .await
        .unwrap()
        .expect("the wal holds the start point");
        let start = domain.take();

        pin_mut!(s);
        for _ in 0..=30 {
            s.next().await.unwrap().unwrap();
        }
        let catch_up = domain.take();

        assert_eq!(
            [("start", start), ("catch_up", catch_up)],
            [("start", (0, true)), ("catch_up", (0, true))]
        );
    }

    #[tokio::test]
    async fn test_stream_unknown_intersect() {
        let domain = ToyDomain::new(None, None);

        for i in 0..=10 {
            let (_, block) = make_conway_block(i * 10);

            use dolos_core::SyncExt;
            domain.roll_forward(block).unwrap();
        }

        // this point was never rolled forward, so the domain can't intersect
        // it.
        let unknown_point = make_conway_block(9999).0;

        let result = ChainStream::start::<ToyDomain, CancelTokenImpl>(
            domain,
            vec![unknown_point],
            CancelTokenImpl(CancellationToken::new()),
        )
        .await;

        assert!(result.unwrap().is_none());
    }
}
