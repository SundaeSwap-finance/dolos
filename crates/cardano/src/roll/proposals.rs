use std::collections::{BTreeMap, HashMap};

use dolos_core::{ChainError, Genesis, TxoRef};
use pallas::{
    codec::utils::Bytes,
    ledger::{
        primitives::{conway::GovActionId, Epoch, ExUnitPrices, RationalNumber},
        traverse::{
            governance::ParamRead, MultiEraBlock, MultiEraGovAction, MultiEraGovActionKind,
            MultiEraParamUpdate, MultiEraProposal, MultiEraTx, MultiEraUpdate,
        },
    },
};

use super::WorkDeltas;
use crate::{
    owned::OwnedMultiEraOutput, pallas_extras, roll::BlockVisitor, GovPurpose, NewProposalV2,
    PParamValue, PParamsSet, ProposalAction, VoteCast,
};

macro_rules! map_shared_pparam {
    ($update:expr, $getter:ident, $set:expr, $variant:ident) => {
        let value = $update.$getter();
        if let Some(value) = value {
            let value = value.try_into().expect("pparam value doesn't fit");
            $set.set(PParamValue::$variant(value));
        }
    };
}

macro_rules! check_shared_pparams {
    ($update:expr, $set:expr, $($getter:ident => $variant:ident),*) => {
        $(
            map_shared_pparam!($update, $getter, $set, $variant);
        )*
    };
}

fn parse_treasury_withdrawals(
    withdrawals: &BTreeMap<Bytes, u64>,
) -> Result<ProposalAction, ChainError> {
    let mut items = vec![];

    for (credential, amount) in withdrawals {
        let credential = pallas_extras::parse_reward_account(credential)
            .ok_or(ChainError::InvalidProposalParams)?;
        let amount = *amount;
        items.push((credential, amount));
    }

    Ok(ProposalAction::TreasuryWithdrawal(items))
}

/// The parameter key a Dijkstra update proposes that the parameter set has no
/// place for, if it proposes one. A key the update leaves alone and a key the
/// update's era does not have are both absent from this answer.
fn unrecordable_param(update: &MultiEraParamUpdate) -> Option<&'static str> {
    macro_rules! first_proposed {
        ($($getter:ident),*) => {
            $(
                if matches!(update.$getter(), ParamRead::Proposed(_)) {
                    return Some(stringify!($getter));
                }
            )*
        };
    }

    first_proposed!(
        max_ref_script_size_per_block,
        max_ref_script_size_per_tx,
        ref_script_cost_stride,
        ref_script_cost_multiplier,
        max_pledge_leverage,
        min_pool_margin,
        leios_announcement_period_length,
        leios_vote_period_length,
        leios_diffusion_period_length,
        leios_committee_size,
        leios_quorum_stake_threshold,
        max_endorser_block_references_size,
        max_endorser_block_txs_size,
        max_endorser_block_execution_units,
        max_ref_script_size_per_endorser_block
    );

    None
}

/// The cost models an update proposes under the keys no field names. The era
/// neutral cost model type names four languages and leaves the wildcard keys
/// out, and Conway reads a PlutusV4 model into the wildcard, so each era's own
/// type is asked for it.
fn wildcard_cost_models(
    update: &MultiEraParamUpdate,
) -> Result<BTreeMap<u64, Vec<i64>>, ChainError> {
    if let Some(conway) = update.as_conway() {
        return Ok(conway
            .cost_models_for_script_languages
            .as_ref()
            .map(|models| models.unknown.clone())
            .unwrap_or_default());
    }

    if let Some(dijkstra) = update.as_dijkstra() {
        return Ok(dijkstra
            .cost_models_for_script_languages
            .as_ref()
            .map(|models| models.unknown.clone())
            .unwrap_or_default());
    }

    Err(ChainError::UnrecordableProposalPart(
        "cost models of an era this node does not read".to_string(),
    ))
}

fn param_update_to_pparamset(update: &MultiEraParamUpdate) -> Result<PParamsSet, ChainError> {
    if let Some(name) = unrecordable_param(update) {
        return Err(ChainError::UnrecordableProposalPart(format!(
            "protocol parameter {name}"
        )));
    }

    let mut set = PParamsSet::default();

    check_shared_pparams! {
        update,
        set,

        minfee_a => MinFeeA,
        minfee_b => MinFeeB,
        max_block_body_size => MaxBlockBodySize,
        max_transaction_size => MaxTransactionSize,
        max_block_header_size => MaxBlockHeaderSize,
        key_deposit => KeyDeposit,
        pool_deposit => PoolDeposit,
        desired_number_of_stake_pools => DesiredNumberOfStakePools,
        ada_per_utxo_byte => MinUtxoValue,
        min_pool_cost => MinPoolCost,
        expansion_rate => ExpansionRate,
        treasury_growth_rate => TreasuryGrowthRate,
        maximum_epoch => MaximumEpoch,
        pool_pledge_influence => PoolPledgeInfluence,
        ada_per_utxo_byte => AdaPerUtxoByte,
        max_value_size => MaxValueSize,
        collateral_percentage => CollateralPercentage,
        max_collateral_inputs => MaxCollateralInputs,
        pool_voting_thresholds => PoolVotingThresholds,
        drep_voting_thresholds => DrepVotingThresholds,
        min_committee_size => MinCommitteeSize,
        committee_term_limit => CommitteeTermLimit,
        governance_action_validity_period => GovernanceActionValidityPeriod,
        governance_action_deposit => GovernanceActionDeposit,
        drep_deposit => DrepDeposit,
        drep_inactivity_period => DrepInactivityPeriod
    };

    // TODO: these are special cases where we don't have automatic type mappings. We
    // should fix this at the Pallas level.

    if let Some(updated) = update.max_tx_ex_units() {
        let value = PParamValue::MaxTxExUnits(pallas::ledger::primitives::ExUnits {
            mem: updated.mem,
            steps: updated.steps,
        });

        set.set(value);
    }

    if let Some(updated) = update.max_block_ex_units() {
        let value = PParamValue::MaxBlockExUnits(pallas::ledger::primitives::ExUnits {
            mem: updated.mem,
            steps: updated.steps,
        });

        set.set(value);
    }

    if let Some(updated) = update.minfee_refscript_cost_per_byte() {
        let value = PParamValue::MinFeeRefScriptCostPerByte(RationalNumber {
            numerator: updated.numerator,
            denominator: updated.denominator,
        });

        set.set(value);
    }

    if let Some(updated) = update.execution_costs() {
        let value = PParamValue::ExecutionCosts(ExUnitPrices {
            mem_price: updated.mem_price.clone(),
            step_price: updated.step_price.clone(),
        });

        set.set(value);
    }

    if let Some(updated) = update.cost_models_for_script_languages() {
        if let Some(v1) = updated.plutus_v1 {
            set.set(PParamValue::CostModelsPlutusV1(v1));
        }

        if let Some(v2) = updated.plutus_v2 {
            set.set(PParamValue::CostModelsPlutusV2(v2));
        }

        if let Some(v3) = updated.plutus_v3 {
            set.set(PParamValue::CostModelsPlutusV3(v3));
        }

        let mut wildcard = wildcard_cost_models(update)?;

        // The parameter set has no field for a PlutusV4 model and holds it
        // under key 3 of the wildcard map, where Conway's own type carries it.
        if let Some(v4) = updated.plutus_v4 {
            wildcard.insert(pallas_extras::PLUTUS_V4_COST_MODEL_KEY, v4);
        }

        if !wildcard.is_empty() {
            set.set(PParamValue::CostModelsUnknown(wildcard));
        }
    }

    Ok(set)
}

macro_rules! map_pre_conway_pparam {
    ($update:expr, $getter:ident, $set:expr, $variant:ident) => {
        let value = $update.$getter().clone();
        if let Some(value) = value.first().cloned() {
            let value = value.try_into().expect("pparam value doesn't fit");
            $set.set(PParamValue::$variant(value));
        }
    };
}

macro_rules! check_pre_conway_pparams {
    ($update:expr, $set:expr, $($getter:ident => $variant:ident),*) => {
        $(
            map_pre_conway_pparam!($update, $getter, $set, $variant);
        )*
    };
}

fn pre_conway_to_pparamset(update: &MultiEraUpdate) -> PParamsSet {
    let mut set = PParamsSet::default();

    check_pre_conway_pparams! {
        update,
        set,

        all_proposed_minfee_a => MinFeeA,
        all_proposed_minfee_b => MinFeeB,
        all_proposed_max_block_body_size => MaxBlockBodySize,
        all_proposed_max_transaction_size => MaxTransactionSize,
        all_proposed_max_block_header_size => MaxBlockHeaderSize,
        all_proposed_key_deposit => KeyDeposit,
        all_proposed_pool_deposit => PoolDeposit,
        all_proposed_desired_number_of_stake_pools => DesiredNumberOfStakePools,
        all_proposed_protocol_version => ProtocolVersion,
        all_proposed_ada_per_utxo_byte => MinUtxoValue,
        all_proposed_min_pool_cost => MinPoolCost,
        all_proposed_expansion_rate => ExpansionRate,
        all_proposed_treasury_growth_rate => TreasuryGrowthRate,
        all_proposed_maximum_epoch => MaximumEpoch,
        all_proposed_pool_pledge_influence => PoolPledgeInfluence,
        all_proposed_decentralization_constant => DecentralizationConstant,
        all_proposed_extra_entropy => ExtraEntropy,
        all_proposed_ada_per_utxo_byte => AdaPerUtxoByte,
        all_proposed_execution_costs => ExecutionCosts,
        all_proposed_max_tx_ex_units => MaxTxExUnits,
        all_proposed_max_block_ex_units => MaxBlockExUnits,
        all_proposed_max_value_size => MaxValueSize,
        all_proposed_collateral_percentage => CollateralPercentage,
        all_proposed_max_collateral_inputs => MaxCollateralInputs,
        all_proposed_pool_voting_thresholds => PoolVotingThresholds,
        all_proposed_drep_voting_thresholds => DrepVotingThresholds,
        all_proposed_min_committee_size => MinCommitteeSize,
        all_proposed_committee_term_limit => CommitteeTermLimit,
        all_proposed_governance_action_validity_period => GovernanceActionValidityPeriod,
        all_proposed_governance_action_deposit => GovernanceActionDeposit,
        all_proposed_drep_deposit => DrepDeposit,
        all_proposed_drep_inactivity_period => DrepInactivityPeriod,
        all_proposed_minfee_refscript_cost_per_byte => MinFeeRefScriptCostPerByte
    };

    if let Some((major, minor, _)) = update.byron_proposed_block_version() {
        set.set(PParamValue::ProtocolVersion((major.into(), minor.into())));
    }

    if let Some(cm) = update.alonzo_first_proposed_cost_models_for_script_languages() {
        if let Some(v1) = cm.get(&pallas::ledger::primitives::alonzo::Language::PlutusV1) {
            set.set(PParamValue::CostModelsPlutusV1(v1.clone()));
        }
    }

    if let Some(cm) = update.babbage_first_proposed_cost_models_for_script_languages() {
        if let Some(v1) = cm.plutus_v1 {
            set.set(PParamValue::CostModelsPlutusV1(v1));
        }
        if let Some(v2) = cm.plutus_v2 {
            set.set(PParamValue::CostModelsPlutusV2(v2));
        }
    }

    if let Some(cm) = update.conway_first_proposed_cost_models_for_script_languages() {
        if let Some(v1) = cm.plutus_v1 {
            set.set(PParamValue::CostModelsPlutusV1(v1));
        }

        if let Some(v2) = cm.plutus_v2 {
            set.set(PParamValue::CostModelsPlutusV2(v2));
        }

        if let Some(v3) = cm.plutus_v3 {
            set.set(PParamValue::CostModelsPlutusV3(v3));
        }

        if !cm.unknown.is_empty() {
            set.set(PParamValue::CostModelsUnknown(cm.unknown));
        }
    }

    set
}

/// Maps a governance action of any era to its dolos representation and the
/// lineage data the action declares: the parent (previous governance action
/// id of the same purpose) and the purpose tree it belongs to.
/// TreasuryWithdrawals and Info have no lineage.
fn parse_gov_action(
    action: &MultiEraGovAction,
) -> Result<(ProposalAction, Option<GovActionId>, Option<GovPurpose>), ChainError> {
    let parent = action.id();

    let (action, purpose) = match action.kind() {
        MultiEraGovActionKind::ParameterChange(_, update, _) => (
            ProposalAction::ParamChange(param_update_to_pparamset(&update)?),
            Some(GovPurpose::PParamUpdate),
        ),
        MultiEraGovActionKind::HardForkInitiation(_, version) => (
            ProposalAction::HardFork(*version),
            Some(GovPurpose::HardFork),
        ),
        MultiEraGovActionKind::TreasuryWithdrawals(withdrawals, _) => {
            (parse_treasury_withdrawals(withdrawals)?, None)
        }
        MultiEraGovActionKind::NoConfidence(_) => {
            (ProposalAction::NoConfidence, Some(GovPurpose::Committee))
        }
        MultiEraGovActionKind::UpdateCommittee(_, to_remove, to_add, threshold) => (
            ProposalAction::UpdateCommittee {
                to_remove: to_remove.to_vec(),
                to_add: to_add
                    .iter()
                    .map(|(cred, epoch)| (cred.clone(), *epoch))
                    .collect(),
                threshold: threshold.clone(),
            },
            Some(GovPurpose::Committee),
        ),
        MultiEraGovActionKind::NewConstitution(_, constitution) => (
            ProposalAction::NewConstitution {
                anchor: constitution.anchor.clone(),
                guardrail_script: constitution.guardrail_script,
            },
            Some(GovPurpose::Constitution),
        ),
        MultiEraGovActionKind::Information => (ProposalAction::Info, None),
        _ => {
            return Err(ChainError::UnrecordableProposalPart(
                "a governance action of a kind this node does not read".to_string(),
            ))
        }
    };

    Ok((action, parent, purpose))
}

#[derive(Clone, Default)]
pub struct ProposalVisitor {
    validity_period: Option<u64>,
    current_epoch: Option<Epoch>,
    network_magic: Option<u32>,
    protocol: Option<u16>,
    pending_votes: Vec<VoteCast>,
}

impl BlockVisitor for ProposalVisitor {
    fn visit_root(
        &mut self,
        _: &mut WorkDeltas,
        _: &MultiEraBlock,
        genesis: &Genesis,
        pparams: &PParamsSet,
        epoch: Epoch,
        _: u64,
        protocol: u16,
    ) -> Result<(), ChainError> {
        self.validity_period = pparams.governance_action_validity_period();
        self.current_epoch = Some(epoch);
        self.network_magic = Some(genesis.network_magic());
        self.protocol = Some(protocol);

        Ok(())
    }

    fn visit_tx(
        &mut self,
        _: &mut WorkDeltas,
        block: &MultiEraBlock,
        tx: &MultiEraTx,
        _: &HashMap<TxoRef, OwnedMultiEraOutput>,
    ) -> Result<(), ChainError> {
        // Phase-2-invalid transactions contribute nothing to governance
        // state: CERTS / GOV only run for valid transactions.
        if !tx.is_valid() {
            return Ok(());
        }

        let Some(voting_procedures) = tx.voting_procedures() else {
            return Ok(());
        };

        for (voter, votes) in voting_procedures.iter() {
            for (gov_action_id, procedure) in votes.iter() {
                self.pending_votes.push(VoteCast::new(
                    gov_action_id.transaction_id,
                    gov_action_id.action_index,
                    voter.clone(),
                    procedure.vote.clone(),
                    block.slot(),
                ));
            }
        }

        Ok(())
    }

    fn visit_update(
        &mut self,
        deltas: &mut WorkDeltas,
        block: &MultiEraBlock,
        tx: Option<&MultiEraTx>,
        update: &MultiEraUpdate,
    ) -> Result<(), ChainError> {
        let action = pre_conway_to_pparamset(update);

        deltas.add_for_entity(NewProposalV2::new(
            block.slot(),
            tx.map(|tx| tx.hash()).unwrap_or_else(|| block.hash()),
            0,
            ProposalAction::ParamChange(action),
            None,
            None,
            self.validity_period,
            self.current_epoch.expect("value set in root"),
            self.network_magic.expect("value set in root"),
            self.protocol.expect("value set in root"),
            // pre-Conway updates carry no Conway lineage or anchor
            None,
            None,
            None,
        ));

        Ok(())
    }

    fn visit_proposal(
        &mut self,
        deltas: &mut WorkDeltas,
        block: &MultiEraBlock,
        tx: &MultiEraTx,
        proposal: &MultiEraProposal,
        idx: usize,
    ) -> Result<(), ChainError> {
        let (action, parent, purpose) = parse_gov_action(&proposal.gov_action())?;

        let reward_account = pallas_extras::parse_reward_account(proposal.reward_account())
            .ok_or(ChainError::InvalidProposalParams)?;

        deltas.add_for_entity(NewProposalV2::new(
            block.slot(),
            tx.hash(),
            idx as u32,
            action,
            Some(proposal.deposit()),
            Some(reward_account),
            self.validity_period,
            self.current_epoch.expect("value set in root"),
            self.network_magic.expect("value set in root"),
            self.protocol.expect("value set in root"),
            parent,
            purpose,
            Some(proposal.anchor().clone()),
        ));

        Ok(())
    }

    fn flush(&mut self, deltas: &mut WorkDeltas) -> Result<(), ChainError> {
        // Votes buffered during `visit_tx` are emitted at block flush so a
        // vote targeting a proposal submitted in the same block (legal in
        // Conway, even within the same tx) lands *after* that proposal's
        // `NewProposalV2` in the per-entity delta ordering.
        for vote in self.pending_votes.drain(..) {
            deltas.add_for_entity(vote);
        }

        Ok(())
    }
}

#[cfg(test)]
mod dijkstra_governance_tests {
    use pallas::codec::{minicbor, utils::Nullable};
    use pallas::ledger::primitives::{dijkstra, ExUnits};

    use super::*;

    /// A Dijkstra update proposing the key given, decoded from a one entry
    /// CBOR map because the type has no Default.
    fn dijkstra_update(key: u64, value: &[u8]) -> dijkstra::ProtocolParamUpdate {
        let mut bytes = vec![0xa1];
        bytes.extend(minicbor::to_vec(key).unwrap());
        bytes.extend_from_slice(value);

        minicbor::decode(&bytes).expect("the one key update does not decode")
    }

    /// A Conway update proposing the key given, decoded from a one entry
    /// CBOR map.
    fn conway_update(
        key: u64,
        value: &[u8],
    ) -> pallas::ledger::primitives::conway::ProtocolParamUpdate {
        let mut bytes = vec![0xa1];
        bytes.extend(minicbor::to_vec(key).unwrap());
        bytes.extend_from_slice(value);

        minicbor::decode(&bytes).expect("the one key update does not decode")
    }

    fn parameter_change(update: dijkstra::ProtocolParamUpdate) -> dijkstra::GovAction {
        dijkstra::GovAction::ParameterChange(None, Box::new(update), None)
    }

    /// Every key the Dijkstra era added, with a value of its own type and the
    /// name the refusal has to carry.
    fn dijkstra_only_keys() -> Vec<(u64, &'static str, Vec<u8>)> {
        let count = minicbor::to_vec(500u64).unwrap();
        let ratio = minicbor::to_vec(RationalNumber {
            numerator: 1,
            denominator: 2,
        })
        .unwrap();
        let nullable_ratio = minicbor::to_vec(Nullable::Some(RationalNumber {
            numerator: 1,
            denominator: 2,
        }))
        .unwrap();
        let units = minicbor::to_vec(ExUnits { mem: 10, steps: 10 }).unwrap();

        vec![
            (34, "max_ref_script_size_per_block", count.clone()),
            (35, "max_ref_script_size_per_tx", count.clone()),
            (36, "ref_script_cost_stride", count.clone()),
            (37, "ref_script_cost_multiplier", ratio.clone()),
            (38, "max_pledge_leverage", nullable_ratio),
            (39, "min_pool_margin", ratio.clone()),
            (40, "leios_announcement_period_length", count.clone()),
            (41, "leios_vote_period_length", count.clone()),
            (42, "leios_diffusion_period_length", count.clone()),
            (43, "leios_committee_size", count.clone()),
            (44, "leios_quorum_stake_threshold", ratio),
            (45, "max_endorser_block_references_size", count.clone()),
            (46, "max_endorser_block_txs_size", count.clone()),
            (47, "max_endorser_block_execution_units", units),
            (48, "max_ref_script_size_per_endorser_block", count),
        ]
    }

    /// The must-fire case. The parameter set has no field for any of the keys
    /// the Dijkstra era added, so a proposal that sets one has to refuse and
    /// name it rather than record a set the key is missing from.
    #[test]
    fn every_dijkstra_only_parameter_key_is_refused_by_name() {
        for (key, name, value) in dijkstra_only_keys() {
            let action = parameter_change(dijkstra_update(key, &value));
            let action = MultiEraGovAction::from_dijkstra(&action);

            let error = parse_gov_action(&action)
                .err()
                .unwrap_or_else(|| panic!("key {key} was recorded"));

            assert!(
                error.to_string().contains(name),
                "key {key} was refused without naming {name}: {error}"
            );
        }
    }

    /// The must-not case. A Dijkstra update of a key every era carries has to
    /// be recorded, or a parameter change on a Dijkstra chain would stop the
    /// node instead of reaching the proposal.
    #[test]
    fn a_dijkstra_update_of_a_shared_parameter_is_recorded() {
        let value = minicbor::to_vec(500u64).unwrap();
        let action = parameter_change(dijkstra_update(0, &value));
        let action = MultiEraGovAction::from_dijkstra(&action);

        let (recorded, parent, purpose) = parse_gov_action(&action).unwrap();

        assert_eq!(parent, None);
        assert_eq!(purpose, Some(GovPurpose::PParamUpdate));

        let ProposalAction::ParamChange(set) = recorded else {
            panic!("a parameter change was recorded as {recorded:?}");
        };

        assert_eq!(set.min_fee_a(), Some(500));
    }

    /// The parameter set holds a PlutusV4 model under key 3 of the wildcard
    /// map, which is where Conway's own cost model type carries it.
    #[test]
    fn a_dijkstra_plutus_v4_cost_model_is_recorded_under_the_wildcard_key() {
        // Key 18 reading a cost model map of key 3 to the vector [1, 2].
        let value = hex::decode("a103820102").unwrap();
        let action = parameter_change(dijkstra_update(18, &value));
        let action = MultiEraGovAction::from_dijkstra(&action);

        let (recorded, _, _) = parse_gov_action(&action).unwrap();

        let ProposalAction::ParamChange(set) = recorded else {
            panic!("a parameter change was recorded as {recorded:?}");
        };

        assert_eq!(
            set.cost_models_unknown(),
            Some(BTreeMap::from([(
                pallas_extras::PLUTUS_V4_COST_MODEL_KEY,
                vec![1, 2]
            )]))
        );
    }

    /// The must-not case for the wildcard map. Reading a Conway update through
    /// the era neutral cost model type, which names four languages and no
    /// wildcard key, must not drop the keys that type has no field for.
    #[test]
    fn a_conway_cost_model_under_a_wildcard_key_is_kept() {
        // Key 18 reading a cost model map of key 5 to the vector [1, 2].
        let value = hex::decode("a105820102").unwrap();
        let action = pallas::ledger::primitives::conway::GovAction::ParameterChange(
            None,
            Box::new(conway_update(18, &value)),
            None,
        );
        let action = MultiEraGovAction::from_conway(&action);

        let (recorded, _, _) = parse_gov_action(&action).unwrap();

        let ProposalAction::ParamChange(set) = recorded else {
            panic!("a parameter change was recorded as {recorded:?}");
        };

        assert_eq!(
            set.cost_models_unknown(),
            Some(BTreeMap::from([(5, vec![1, 2])]))
        );
    }
}
