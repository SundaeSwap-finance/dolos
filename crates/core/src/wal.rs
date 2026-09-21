use itertools::Itertools as _;

use super::*;

#[trait_variant::make(Send)]
pub trait WalStore: Clone + Send + Sync + 'static {
    type Delta: EntityDelta;
    type LogIterator<'a>: DoubleEndedIterator<Item = LogEntry<Self::Delta>> + Sized + Sync + Send;
    type BlockIterator<'a>: DoubleEndedIterator<Item = (ChainPoint, RawBlock)> + Sized + Sync + Send;

    fn reset_to(&self, point: &ChainPoint) -> Result<(), WalError>;

    fn truncate_front(&self, after: &ChainPoint) -> Result<(), WalError>;

    fn prune_history(&self, max_slots: u64, max_prune: Option<u64>) -> Result<bool, WalError>;

    fn locate_point(&self, around: BlockSlot) -> Result<Option<ChainPoint>, WalError>;

    fn read_entry(&self, key: &ChainPoint) -> Result<Option<LogValue<Self::Delta>>, WalError>;

    fn iter_logs<'a>(
        &self,
        start: Option<ChainPoint>,
        end: Option<ChainPoint>,
    ) -> Result<Self::LogIterator<'a>, WalError>;

    fn iter_blocks<'a>(
        &self,
        start: Option<ChainPoint>,
        end: Option<ChainPoint>,
    ) -> Result<Self::BlockIterator<'a>, WalError>;

    fn read_sparse(
        &self,
        points: &[ChainPoint],
    ) -> Result<Vec<Option<LogValue<Self::Delta>>>, WalError> {
        points.iter().map(|p| self.read_entry(p)).try_collect()
    }

    fn append_entries(&self, logs: Vec<LogEntry<Self::Delta>>) -> Result<(), WalError>;

    fn remove_entries(&mut self, after: &ChainPoint) -> Result<(), WalError>;

    fn contains_point(&self, point: &ChainPoint) -> Result<bool, WalError> {
        let entry = self.read_entry(point)?;
        Ok(entry.is_some())
    }

    /// Asserts that a chain point exists in the WAL and returns the sequence
    ///
    /// Similar to `locate_point` but it expects a point to be found or
    /// otherwise return a NotFound error.
    fn assert_point(&self, point: &ChainPoint) -> Result<(), WalError> {
        let contains = self.contains_point(point)?;

        if !contains {
            return Err(WalError::PointNotFound(point.clone()));
        }

        Ok(())
    }

    fn iter_all<'a>(&'a self) -> Result<Self::LogIterator<'a>, WalError> {
        self.iter_logs(None, None)
    }

    fn find_start(&self) -> Result<Option<LogEntry<Self::Delta>>, WalError> {
        let start = self.iter_all()?.next();

        Ok(start)
    }

    fn find_tip(&self) -> Result<Option<LogEntry<Self::Delta>>, WalError> {
        let tip = self.iter_all()?.next_back();

        Ok(tip)
    }

    fn intersect_candidates(&self, max_items: usize) -> Result<Vec<ChainPoint>, WalError> {
        let mut iter = self.iter_all()?.rev();

        let mut out = Vec::with_capacity(max_items);

        // crawl the wal exponentially
        while let Some((point, _)) = iter.next() {
            // Skip synthetic entries with zero hash (from reset_to with ChainPoint::Slot)
            if !point.is_fully_defined() {
                continue;
            }

            out.push(point);

            if out.len() >= max_items {
                break;
            }

            // skip exponentially
            let skip = 2usize.pow(out.len() as u32) - 1;
            for _ in 0..skip {
                iter.next();
            }
        }

        Ok(out)
    }

    fn find_intersect(
        &self,
        intersect: &[ChainPoint],
    ) -> Result<Option<LogEntry<Self::Delta>>, WalError> {
        for candidate in intersect {
            if let Some(entry) = self.read_entry(candidate)? {
                return Ok(Some((candidate.clone(), entry)));
            }
        }

        Ok(None)
    }
}

/// Whether two points name the same block.
///
/// Origin names no block at all, so it never matches one. Its slot reads as
/// zero, which would otherwise make it name the block at slot zero.
///
/// A point that has no hash names every block at its slot, which is the most a
/// caller that gave no hash can be held to. The two points are matched by
/// shape rather than compared with `==`, because `==` on a point answers
/// differently depending on which side has the hash.
fn names_the_same_block(a: &ChainPoint, b: &ChainPoint) -> bool {
    match (a, b) {
        (ChainPoint::Origin, _) | (_, ChainPoint::Origin) => false,
        (ChainPoint::Specific(_, a_hash), ChainPoint::Specific(_, b_hash)) => {
            a.slot() == b.slot() && a_hash == b_hash
        }
        _ => a.slot() == b.slot(),
    }
}

/// The blocks a caller resuming at `from` has not applied.
///
/// A wal block iteration opened at a point starts with that point, and the
/// caller named that point as the block it last applied, so it is dropped.
pub fn blocks_after<'a>(
    from: Option<&'a ChainPoint>,
    blocks: impl Iterator<Item = (ChainPoint, RawBlock)> + 'a,
) -> impl Iterator<Item = (ChainPoint, RawBlock)> + 'a {
    blocks.filter(move |(point, _)| match from {
        Some(from) => !names_the_same_block(from, point),
        None => true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(slot: u64, tag: u8) -> (ChainPoint, RawBlock) {
        (
            ChainPoint::Specific(slot, [tag; 32].into()),
            Arc::new(vec![tag]),
        )
    }

    fn slots(blocks: impl Iterator<Item = (ChainPoint, RawBlock)>) -> Vec<u64> {
        blocks.map(|(point, _)| point.slot()).collect()
    }

    #[test]
    fn the_point_the_caller_named_is_dropped() {
        let from = ChainPoint::Specific(5, [1u8; 32].into());
        let page = [block(5, 1), block(6, 2), block(7, 3)];

        assert_eq!(slots(blocks_after(Some(&from), page.into_iter())), [6, 7]);
    }

    #[test]
    fn a_point_with_no_hash_names_the_block_at_that_slot() {
        let from = ChainPoint::Slot(5);
        let page = [block(5, 1), block(6, 2), block(7, 3)];

        assert_eq!(slots(blocks_after(Some(&from), page.into_iter())), [6, 7]);
    }

    #[test]
    fn a_different_block_at_the_same_slot_is_kept() {
        let from = ChainPoint::Specific(5, [9u8; 32].into());
        let page = [block(5, 1), block(6, 2)];

        assert_eq!(slots(blocks_after(Some(&from), page.into_iter())), [5, 6]);
    }

    #[test]
    fn every_block_is_kept_when_the_caller_names_no_point() {
        let page = [block(5, 1), block(6, 2)];

        assert_eq!(slots(blocks_after(None, page.into_iter())), [5, 6]);
    }

    #[test]
    fn a_caller_resuming_at_origin_keeps_the_block_at_slot_zero() {
        let from = ChainPoint::Origin;
        let page = [block(0, 1), block(5, 2)];

        assert_eq!(slots(blocks_after(Some(&from), page.into_iter())), [0, 5]);
    }
}
