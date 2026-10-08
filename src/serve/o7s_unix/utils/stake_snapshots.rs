use crate::prelude::*;
use dolos_cardano::pool_stakes::{load_pool_stakes, PoolStakes};
use dolos_cardano::{load_epoch, load_era_summary, EraProtocol, PoolHash};
use pallas::codec::minicbor;
use pallas::codec::utils::{AnyCbor, Bytes, KeyValuePairs, TagWrap};
use pallas::network::miniprotocols::localstate::queries_v16 as q16;
use pallas::network::miniprotocols::localtxsubmission::SMaybe;
use std::collections::BTreeSet;
use tracing::debug;

pub fn build_stake_snapshots_response<D: Domain>(
    domain: &D,
    pools_filter: &SMaybe<q16::Pools>,
) -> Result<AnyCbor, Error> {
    let state = domain.state();

    let chain_summary = load_era_summary::<D>(state)
        .map_err(|e| Error::server(format!("failed to load era summary: {e}")))?;

    let epoch_state = load_epoch::<D>(state)
        .map_err(|e| Error::server(format!("failed to load epoch: {}", e)))?;
    let current_epoch = epoch_state.number;

    let filter_set: Option<BTreeSet<Vec<u8>>> = match pools_filter {
        SMaybe::Some(pools) => {
            let set: BTreeSet<Vec<u8>> = pools.0.iter().map(|p| p.to_vec()).collect();
            Some(set)
        }
        SMaybe::None => None,
    };

    let gather_for_epoch = |epoch: u64| -> Result<PoolStakes, Error> {
        let protocol = EraProtocol::from(chain_summary.era_for_epoch(epoch).protocol);
        let mut stakes = load_pool_stakes::<D>(state, epoch, protocol)
            .map_err(|e| Error::server(format!("failed to load pool stakes: {e}")))?;

        if let Some(filter_set) = &filter_set {
            stakes
                .pools
                .retain(|hash, _| filter_set.contains(hash.as_slice()));
        }

        Ok(stakes)
    };

    let mark = gather_for_epoch(current_epoch.saturating_sub(1))?;
    let set = gather_for_epoch(current_epoch.saturating_sub(2))?;
    let go = gather_for_epoch(current_epoch.saturating_sub(3))?;

    let all_pools: BTreeSet<&PoolHash> = mark
        .pools
        .keys()
        .chain(set.pools.keys())
        .chain(go.pools.keys())
        .collect();

    let stake_of = |stakes: &PoolStakes, hash: &PoolHash| {
        stakes.pools.get(hash).map(|x| x.stake).unwrap_or_default()
    };

    let stake_snapshots: Vec<(Bytes, q16::Stakes)> = all_pools
        .into_iter()
        .map(|hash| {
            let stakes = q16::Stakes {
                snapshot_mark_pool: stake_of(&mark, hash),
                snapshot_set_pool: stake_of(&set, hash),
                snapshot_go_pool: stake_of(&go, hash),
            };
            (hash.to_vec().into(), stakes)
        })
        .collect();

    let (mark_total, set_total, go_total) = (mark.total, set.total, go.total);

    debug!(
        num_pools = stake_snapshots.len(),
        mark_total, set_total, go_total, "returning stake snapshots"
    );

    let response = q16::StakeSnapshots {
        stake_snapshots: KeyValuePairs::Def(stake_snapshots),
        snapshot_stake_mark_total: mark_total,
        snapshot_stake_set_total: set_total,
        snapshot_stake_go_total: go_total,
    };

    let encoded = minicbor::to_vec(response)
        .map_err(|e| Error::server(format!("failed to encode stake snapshots: {e}")))?;

    let wrapped = TagWrap::<Bytes, 24>(encoded.into());
    Ok(AnyCbor::from_encode(vec![wrapped]))
}
