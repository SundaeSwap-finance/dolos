use pallas::ledger::traverse::Era;
use pallas::ledger::traverse::MultiEraBlock;
use pallas::network::miniprotocols::chainsync;

use crate::prelude::*;

fn era_to_header_variant(era: Era) -> u8 {
    match era {
        Era::Byron => 0,
        Era::Shelley => 1,
        Era::Allegra => 2,
        Era::Mary => 3,
        Era::Alonzo => 4,
        Era::Babbage => 5,
        Era::Conway => 6,
        // The chainsync header envelope tag, which is a different number from
        // the block wrapper tag: the wrapper counts Byron twice, so from
        // Shelley on the envelope tag is one lower. Dijkstra is wrapper tag 8
        // and envelope tag 7.
        Era::Dijkstra => 7,
        _ => todo!("don't know how to process era"),
    }
}

fn define_byron_prefix(block: &MultiEraBlock) -> Option<(u8, u64)> {
    match block.era() {
        pallas::ledger::traverse::Era::Byron => {
            if block.header().as_eb().is_some() {
                Some((0, 0))
            } else {
                Some((1, 0))
            }
        }
        _ => None,
    }
}

pub fn header_cbor_to_chainsync(block: RawBlock) -> Result<chainsync::HeaderContent, Error> {
    let block = pallas::ledger::traverse::MultiEraBlock::decode(&block).map_err(Error::parse)?;

    let out = chainsync::HeaderContent {
        variant: era_to_header_variant(block.era()),
        byron_prefix: define_byron_prefix(&block),
        cbor: block.header().cbor().to_vec(),
    };

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::era_to_header_variant;
    use pallas::ledger::traverse::{probe, Era};

    /// A two element array whose first element is the block wrapper tag, which
    /// is all of a block that the era probe reads.
    fn wrapper(tag: u8) -> Vec<u8> {
        vec![0x82, tag, 0x00]
    }

    #[test]
    fn dijkstra_is_block_wrapper_tag_eight_and_chainsync_header_variant_seven() {
        assert!(matches!(
            probe::block_era(&wrapper(8)),
            probe::Outcome::Matched(Era::Dijkstra)
        ));
        assert_eq!(era_to_header_variant(Era::Dijkstra), 7);
    }

    #[test]
    fn conway_keeps_wrapper_tag_seven_and_header_variant_six() {
        assert!(matches!(
            probe::block_era(&wrapper(7)),
            probe::Outcome::Matched(Era::Conway)
        ));
        assert_eq!(era_to_header_variant(Era::Conway), 6);
    }

    #[test]
    fn a_wrapper_tag_past_dijkstra_matches_no_era() {
        assert!(matches!(
            probe::block_era(&wrapper(9)),
            probe::Outcome::Inconclusive
        ));
    }
}
