use dolos_core::mempool::MultiEraEvalReport;
use pallas::ledger::traverse::{MultiEraRedeemer, MultiEraRedeemerTag, MultiEraTx};

use crate::prelude::*;

/// A transaction's script evaluation with each of the transaction's redeemers
/// under its tag and index.
pub(super) struct TxEvaluation<R> {
    pub report: MultiEraEvalReport,
    pub redeemers: Vec<(MultiEraRedeemerTag, u32, R)>,
}

impl<R> TxEvaluation<R> {
    /// The redeemer the transaction holds under a tag and index.
    pub fn redeemer(&self, tag: MultiEraRedeemerTag, index: u32) -> Option<&R> {
        self.redeemers
            .iter()
            .find(|(t, i, _)| (*t, *i) == (tag, index))
            .map(|(_, _, r)| r)
    }
}

/// Estimates a submitted transaction's script costs using ledger and mempool
/// inputs, and gives them with the transaction's redeemers to `answer`.
pub(super) fn evaluate_tx<D: Domain, T>(
    domain: &D,
    chain: &D::Chain,
    cbor: &[u8],
    answer: impl FnOnce(Result<TxEvaluation<MultiEraRedeemer<'_>>, DomainError>) -> T,
) -> T {
    let (report, tx) = match estimate(domain, chain, cbor) {
        Ok(estimated) => estimated,
        Err(e) => return answer(Err(e)),
    };
    let redeemers = tx
        .redeemers()
        .into_iter()
        .map(|r| (r.multi_era_tag(), r.index(), r))
        .collect();
    answer(Ok(TxEvaluation { report, redeemers }))
}

/// A submitted transaction's script costs and the transaction decoded in the
/// era of the active parameters.
fn estimate<'a, D: Domain>(
    domain: &D,
    chain: &D::Chain,
    cbor: &'a [u8],
) -> Result<(MultiEraEvalReport, MultiEraTx<'a>), DomainError> {
    let report = domain.estimate_tx(chain, cbor)?;
    let active = dolos_cardano::load_effective_pparams::<D>(domain.state())?;
    let era = dolos_cardano::validate::submission_era(active.ensure_protocol_version()?)?;
    let tx = dolos_cardano::validate::decode_submission(era, cbor)?;
    Ok((report, tx))
}
