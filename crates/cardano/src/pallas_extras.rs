use std::ops::Deref as _;

use dolos_core::BlockSlot;
use pallas::crypto::hash::Hash;
use pallas::ledger::addresses::{
    Address, Network, ShelleyAddress, ShelleyDelegationPart, StakeAddress, StakePayload,
};
use pallas::ledger::primitives::alonzo::MoveInstantaneousReward;
use pallas::ledger::primitives::conway::{
    CostModels, DRep, DRepVotingThresholds, PoolVotingThresholds,
};
use pallas::ledger::primitives::{
    alonzo::Certificate as AlonzoCert, conway::Certificate as ConwayCert,
    dijkstra::Certificate as DijkstraCert, PoolMetadata, RationalNumber, Relay, StakeCredential,
};
use pallas::ledger::primitives::{Epoch, ExUnitPrices, ExUnits, Nonce, NonceVariant};
use pallas::ledger::traverse::{MultiEraCert, MultiEraScriptRef, MultiEraTx};
use serde::{Deserialize, Serialize};

use crate::eras::ChainSummary;
use crate::{hacks, Lovelace};

/// A Dijkstra certificate expressed as the Conway certificate it means.
///
/// Every Dijkstra variant has a Conway counterpart carrying the same values.
/// Dijkstra drops Conway's two legacy stake variants and adds nothing, apart
/// from an optional Leios key on the pool parameters. That key is the one
/// thing this does not carry across, because no accessor in this module has a
/// field to put it in and [`MultiEraPoolRegistration`] would have to grow one
/// first.
///
/// Written with no catch-all so that a variant added to either era becomes a
/// compile error here, rather than a certificate that disappears.
fn dijkstra_cert_as_conway(cert: &DijkstraCert) -> ConwayCert {
    match cert {
        DijkstraCert::StakeDelegation(cred, pool) => {
            ConwayCert::StakeDelegation(cred.clone(), *pool)
        }
        DijkstraCert::PoolRegistration {
            operator,
            vrf_keyhash,
            bls_key: _,
            pledge,
            cost,
            margin,
            reward_account,
            pool_owners,
            relays,
            pool_metadata,
        } => ConwayCert::PoolRegistration {
            operator: *operator,
            vrf_keyhash: *vrf_keyhash,
            pledge: *pledge,
            cost: *cost,
            margin: margin.clone(),
            reward_account: reward_account.clone(),
            // The Dijkstra era has its own set type, which records whether the
            // bytes carried the 258 tag. Conway's set has no field for that, so
            // the owners become a plain vector.
            pool_owners: pool_owners.to_vec().into(),
            relays: relays.clone(),
            pool_metadata: pool_metadata.clone(),
        },
        DijkstraCert::PoolRetirement(pool, epoch) => ConwayCert::PoolRetirement(*pool, *epoch),
        DijkstraCert::Reg(cred, coin) => ConwayCert::Reg(cred.clone(), *coin),
        DijkstraCert::UnReg(cred, coin) => ConwayCert::UnReg(cred.clone(), *coin),
        DijkstraCert::VoteDeleg(cred, drep) => ConwayCert::VoteDeleg(cred.clone(), drep.clone()),
        DijkstraCert::StakeVoteDeleg(cred, pool, drep) => {
            ConwayCert::StakeVoteDeleg(cred.clone(), *pool, drep.clone())
        }
        DijkstraCert::StakeRegDeleg(cred, pool, coin) => {
            ConwayCert::StakeRegDeleg(cred.clone(), *pool, *coin)
        }
        DijkstraCert::VoteRegDeleg(cred, drep, coin) => {
            ConwayCert::VoteRegDeleg(cred.clone(), drep.clone(), *coin)
        }
        DijkstraCert::StakeVoteRegDeleg(cred, pool, drep, coin) => {
            ConwayCert::StakeVoteRegDeleg(cred.clone(), *pool, drep.clone(), *coin)
        }
        DijkstraCert::AuthCommitteeHot(cold, hot) => {
            ConwayCert::AuthCommitteeHot(cold.clone(), hot.clone())
        }
        DijkstraCert::ResignCommitteeCold(cold, anchor) => {
            ConwayCert::ResignCommitteeCold(cold.clone(), anchor.clone())
        }
        DijkstraCert::RegDRepCert(cred, coin, anchor) => {
            ConwayCert::RegDRepCert(cred.clone(), *coin, anchor.clone())
        }
        DijkstraCert::UnRegDRepCert(cred, coin) => ConwayCert::UnRegDRepCert(cred.clone(), *coin),
        DijkstraCert::UpdateDRepCert(cred, anchor) => {
            ConwayCert::UpdateDRepCert(cred.clone(), anchor.clone())
        }
    }
}

/// The Conway view of a certificate, for every era whose certificates have a
/// Conway shape.
///
/// The accessors below all read Conway-shaped certificates, and there are now
/// two eras that produce them. Going through one place means a new era is
/// added once rather than in eleven matches, and that missing it shows up as
/// every certificate on the chain vanishing at once rather than one accessor
/// quietly answering nothing.
pub fn as_conway_cert<'a>(cert: &'a MultiEraCert) -> Option<std::borrow::Cow<'a, ConwayCert>> {
    match cert {
        MultiEraCert::Conway(x) => Some(std::borrow::Cow::Borrowed(x.deref().deref())),
        MultiEraCert::Dijkstra(x) => Some(std::borrow::Cow::Owned(dijkstra_cert_as_conway(
            x.deref().deref(),
        ))),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiEraPoolRegistration {
    pub operator: Hash<28>,
    pub vrf_keyhash: Hash<32>,
    pub pledge: u64,
    pub cost: u64,
    pub margin: RationalNumber,
    pub reward_account: Vec<u8>,
    pub pool_owners: Vec<Hash<28>>,
    pub relays: Vec<Relay>,
    pub pool_metadata: Option<PoolMetadata>,
}

pub fn cert_as_pool_registration(cert: &MultiEraCert) -> Option<MultiEraPoolRegistration> {
    match cert {
        MultiEraCert::AlonzoCompatible(cow) => match cow.deref().deref() {
            AlonzoCert::PoolRegistration {
                operator,
                vrf_keyhash,
                pledge,
                cost,
                margin,
                reward_account,
                pool_owners,
                relays,
                pool_metadata,
            } => Some(MultiEraPoolRegistration {
                operator: *operator,
                vrf_keyhash: *vrf_keyhash,
                pledge: *pledge,
                cost: *cost,
                margin: margin.clone(),
                reward_account: reward_account.to_vec(),
                pool_owners: pool_owners.clone(),
                relays: relays.clone(),
                pool_metadata: pool_metadata.clone(),
            }),
            _ => None,
        },
        _ => match as_conway_cert(cert)?.as_ref() {
            ConwayCert::PoolRegistration {
                operator,
                vrf_keyhash,
                pledge,
                cost,
                margin,
                reward_account,
                pool_owners,
                relays,
                pool_metadata,
            } => Some(MultiEraPoolRegistration {
                operator: *operator,
                vrf_keyhash: *vrf_keyhash,
                pledge: *pledge,
                cost: *cost,
                margin: margin.clone(),
                reward_account: reward_account.to_vec(),
                pool_owners: Vec::from_iter(pool_owners.iter().cloned()),
                relays: relays.clone(),
                pool_metadata: pool_metadata.clone(),
            }),
            _ => None,
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiEraPoolRetirement {
    pub operator: Hash<28>,
    pub epoch: Epoch,
}

pub fn cert_as_pool_retirement(cert: &MultiEraCert) -> Option<MultiEraPoolRetirement> {
    match cert {
        MultiEraCert::AlonzoCompatible(cow) => match cow.deref().deref() {
            AlonzoCert::PoolRetirement(operator, epoch) => Some(MultiEraPoolRetirement {
                operator: *operator,
                epoch: *epoch,
            }),
            _ => None,
        },
        _ => match as_conway_cert(cert)?.as_ref() {
            ConwayCert::PoolRetirement(operator, epoch) => Some(MultiEraPoolRetirement {
                operator: *operator,
                epoch: *epoch,
            }),
            _ => None,
        },
    }
}

pub struct MultiEraVoteDelegation {
    pub delegator: StakeCredential,
    pub drep: DRep,
}

pub fn cert_as_vote_delegation(cert: &MultiEraCert) -> Option<MultiEraVoteDelegation> {
    match as_conway_cert(cert)?.as_ref() {
        ConwayCert::VoteDeleg(delegator, drep) => Some(MultiEraVoteDelegation {
            delegator: delegator.clone(),
            drep: drep.clone(),
        }),
        ConwayCert::VoteRegDeleg(delegator, drep, _) => Some(MultiEraVoteDelegation {
            delegator: delegator.clone(),
            drep: drep.clone(),
        }),
        ConwayCert::StakeVoteRegDeleg(delegator, _, drep, _) => Some(MultiEraVoteDelegation {
            delegator: delegator.clone(),
            drep: drep.clone(),
        }),
        ConwayCert::StakeVoteDeleg(delegator, _, drep) => Some(MultiEraVoteDelegation {
            delegator: delegator.clone(),
            drep: drep.clone(),
        }),
        _ => None,
    }
}

pub struct MultiEraDRepRegistration {
    pub cred: StakeCredential,
    pub deposit: Lovelace,
}

pub fn cert_as_drep_registration(cert: &MultiEraCert) -> Option<MultiEraDRepRegistration> {
    match as_conway_cert(cert)?.as_ref() {
        ConwayCert::RegDRepCert(cred, deposit, _) => Some(MultiEraDRepRegistration {
            cred: cred.clone(),
            deposit: *deposit,
        }),
        _ => None,
    }
}

pub type MultiEraDRepUnRegistration = MultiEraDRepRegistration;

pub fn cert_as_drep_unregistration(cert: &MultiEraCert) -> Option<MultiEraDRepUnRegistration> {
    match as_conway_cert(cert)?.as_ref() {
        ConwayCert::UnRegDRepCert(cred, deposit) => Some(MultiEraDRepRegistration {
            cred: cred.clone(),
            deposit: *deposit,
        }),
        _ => None,
    }
}

pub struct MultiEraCommitteeAuth {
    pub cold: StakeCredential,
    pub hot: StakeCredential,
}

pub fn cert_as_committee_auth(cert: &MultiEraCert) -> Option<MultiEraCommitteeAuth> {
    match as_conway_cert(cert)?.as_ref() {
        ConwayCert::AuthCommitteeHot(cold, hot) => Some(MultiEraCommitteeAuth {
            cold: cold.clone(),
            hot: hot.clone(),
        }),
        _ => None,
    }
}

pub struct MultiEraCommitteeResign {
    pub cold: StakeCredential,
    pub anchor: Option<pallas::ledger::primitives::conway::Anchor>,
}

pub fn cert_as_committee_resign(cert: &MultiEraCert) -> Option<MultiEraCommitteeResign> {
    match as_conway_cert(cert)?.as_ref() {
        ConwayCert::ResignCommitteeCold(cold, anchor) => Some(MultiEraCommitteeResign {
            cold: cold.clone(),
            anchor: anchor.clone(),
        }),
        _ => None,
    }
}

#[derive(Debug)]
pub struct MultiEraStakeDelegation {
    pub delegator: StakeCredential,
    pub pool: Hash<28>,
}

pub fn cert_as_stake_delegation(cert: &MultiEraCert) -> Option<MultiEraStakeDelegation> {
    match cert {
        MultiEraCert::AlonzoCompatible(cow) => match cow.deref().deref() {
            AlonzoCert::StakeDelegation(delegator, pool) => Some(MultiEraStakeDelegation {
                delegator: delegator.clone(),
                pool: *pool,
            }),
            _ => None,
        },
        _ => match as_conway_cert(cert)?.as_ref() {
            ConwayCert::StakeDelegation(delegator, pool) => Some(MultiEraStakeDelegation {
                delegator: delegator.clone(),
                pool: *pool,
            }),
            ConwayCert::StakeRegDeleg(delegator, pool, _) => Some(MultiEraStakeDelegation {
                delegator: delegator.clone(),
                pool: *pool,
            }),
            ConwayCert::StakeVoteRegDeleg(delegator, pool, _, _) => Some(MultiEraStakeDelegation {
                delegator: delegator.clone(),
                pool: *pool,
            }),
            ConwayCert::StakeVoteDeleg(delegator, pool, _) => Some(MultiEraStakeDelegation {
                delegator: delegator.clone(),
                pool: *pool,
            }),
            _ => None,
        },
    }
}

pub fn cert_as_stake_registration(cert: &MultiEraCert) -> Option<StakeCredential> {
    match cert {
        MultiEraCert::AlonzoCompatible(cow) => match cow.deref().deref() {
            AlonzoCert::StakeRegistration(credential) => Some(credential.clone()),
            _ => None,
        },
        _ => match as_conway_cert(cert)?.as_ref() {
            ConwayCert::StakeRegistration(credential) => Some(credential.clone()),
            ConwayCert::Reg(cred, _) => Some(cred.clone()),
            ConwayCert::StakeRegDeleg(cred, _, _) => Some(cred.clone()),
            ConwayCert::VoteRegDeleg(cred, _, _) => Some(cred.clone()),
            ConwayCert::StakeVoteRegDeleg(cred, _, _, _) => Some(cred.clone()),
            _ => None,
        },
    }
}

pub fn cert_as_stake_deregistration(cert: &MultiEraCert) -> Option<StakeCredential> {
    match cert {
        MultiEraCert::AlonzoCompatible(cow) => match cow.deref().deref() {
            AlonzoCert::StakeDeregistration(credential) => Some(credential.clone()),
            _ => None,
        },
        _ => match as_conway_cert(cert)?.as_ref() {
            ConwayCert::StakeDeregistration(credential) => Some(credential.clone()),
            ConwayCert::UnReg(cred, _) => Some(cred.clone()),
            _ => None,
        },
    }
}

/// Move instantaneous rewards were removed by Conway and never came back, so
/// no Conway or Dijkstra certificate can be one and the catch-all here is the
/// right answer rather than a dropped era.
pub fn cert_as_mir_certificate(cert: &MultiEraCert) -> Option<MoveInstantaneousReward> {
    match cert {
        MultiEraCert::AlonzoCompatible(cow) => match cow.deref().deref() {
            AlonzoCert::MoveInstantaneousRewardsCert(mir) => Some(mir.clone()),
            _ => None,
        },
        _ => None,
    }
}

pub fn stake_credential_to_address(network: Network, credential: &StakeCredential) -> StakeAddress {
    match credential {
        StakeCredential::ScriptHash(x) => StakeAddress::new(network, StakePayload::Script(*x)),
        StakeCredential::AddrKeyhash(x) => StakeAddress::new(network, StakePayload::Stake(*x)),
    }
}

pub fn stake_address_to_cred(address: &StakeAddress) -> StakeCredential {
    match address.payload() {
        StakePayload::Stake(x) => StakeCredential::AddrKeyhash(*x),
        StakePayload::Script(x) => StakeCredential::ScriptHash(*x),
    }
}

pub fn shelley_address_to_stake_cred(
    address: &ShelleyAddress,
) -> Option<(StakeCredential, IsPointer)> {
    match address.delegation() {
        ShelleyDelegationPart::Key(x) => Some((StakeCredential::AddrKeyhash(*x), false)),
        ShelleyDelegationPart::Script(x) => Some((StakeCredential::ScriptHash(*x), false)),
        ShelleyDelegationPart::Pointer(x) => hacks::pointers::pointer_to_cred(x).map(|x| (x, true)),
        ShelleyDelegationPart::Null => None,
    }
}

pub fn shelley_address_to_stake_address(address: &ShelleyAddress) -> Option<StakeAddress> {
    match address.delegation() {
        ShelleyDelegationPart::Key(x) => Some(StakeAddress::new(
            address.network(),
            StakePayload::Stake(*x),
        )),
        ShelleyDelegationPart::Script(x) => Some(StakeAddress::new(
            address.network(),
            StakePayload::Script(*x),
        )),
        _ => None,
    }
}

pub type IsPointer = bool;

pub fn address_as_stake_cred(address: &Address) -> Option<(StakeCredential, IsPointer)> {
    match &address {
        Address::Shelley(x) => shelley_address_to_stake_cred(x),
        Address::Stake(x) => Some((stake_address_to_cred(x), false)),
        _ => None,
    }
}

pub fn epoch_boundary(
    chain_summary: &ChainSummary,
    prev_slot: BlockSlot,
    next_slot: BlockSlot,
) -> Option<(Epoch, BlockSlot, Epoch)> {
    let (prev_epoch, _) = chain_summary.slot_epoch(prev_slot);
    let (next_epoch, _) = chain_summary.slot_epoch(next_slot);

    if prev_epoch != next_epoch {
        let boundary = chain_summary.epoch_start(next_epoch);
        Some((prev_epoch, boundary, next_epoch))
    } else {
        None
    }
}

pub fn rupd_boundary(
    stability_window: u64,
    chain_summary: &ChainSummary,
    prev_slot: BlockSlot,
    next_slot: BlockSlot,
) -> Option<BlockSlot> {
    let (prev_epoch, _) = chain_summary.slot_epoch(prev_slot);

    let epoch_start = chain_summary.epoch_start(prev_epoch);

    let boundary = epoch_start + stability_window;

    if prev_slot <= boundary && boundary < next_slot {
        Some(boundary)
    } else {
        None
    }
}

pub fn default_rational_number() -> RationalNumber {
    RationalNumber {
        numerator: 0,
        denominator: 1,
    }
}

pub fn default_pool_voting_thresholds() -> PoolVotingThresholds {
    PoolVotingThresholds {
        motion_no_confidence: default_rational_number(),
        committee_normal: default_rational_number(),
        committee_no_confidence: default_rational_number(),
        hard_fork_initiation: default_rational_number(),
        security_voting_threshold: default_rational_number(),
    }
}

pub fn default_drep_voting_thresholds() -> DRepVotingThresholds {
    DRepVotingThresholds {
        motion_no_confidence: default_rational_number(),
        committee_normal: default_rational_number(),
        committee_no_confidence: default_rational_number(),
        hard_fork_initiation: default_rational_number(),
        pp_network_group: default_rational_number(),
        pp_economic_group: default_rational_number(),
        pp_technical_group: default_rational_number(),
        treasury_withdrawal: default_rational_number(),
        update_constitution: default_rational_number(),
        pp_governance_group: default_rational_number(),
    }
}

pub fn default_nonce() -> Nonce {
    Nonce {
        variant: NonceVariant::NeutralNonce,
        hash: None,
    }
}

pub fn default_ex_units() -> ExUnits {
    ExUnits { mem: 0, steps: 0 }
}

pub fn default_ex_unit_prices() -> ExUnitPrices {
    ExUnitPrices {
        mem_price: default_rational_number(),
        step_price: default_rational_number(),
    }
}

/// The cost model map key that names PlutusV4.
///
/// The Conway cost model type names keys 0 to 2 and reads every further key
/// into its wildcard map, so a PlutusV4 model is found here by key rather than
/// by field.
pub const PLUTUS_V4_COST_MODEL_KEY: u64 = 3;

pub fn default_cost_models() -> CostModels {
    CostModels {
        plutus_v1: None,
        plutus_v2: None,
        plutus_v3: None,
        unknown: Default::default(),
    }
}

/// The language of a script, across every era that can carry one.
///
/// Taken from pallas, which now names the same five languages and is the side
/// that decides what a reference script reports. The type is exhaustive there
/// too, so a language no consumer has a case for is still a compile error
/// rather than a catch-all reporting a real script as absent.
pub use pallas::ledger::traverse::script_ref::ScriptLanguage;

/// A reference script decomposed into everything a consumer needs from it.
///
/// The three fields travel together because they are only correct together.
/// Each language hashes its own tagged serialization over its own bytes, so a
/// hash taken from one variant and bytes taken from another describe no script
/// that exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptRefParts {
    pub language: ScriptLanguage,
    pub hash: Hash<28>,
    pub bytes: Vec<u8>,
}

/// Decompose a reference script from any era into its language, on-chain hash
/// and bytes.
///
/// The language drives the read, because it is the one answer that covers
/// every variant of every era. A native script is the only kind whose bytes
/// are not the script itself, and it is asked for by name.
pub fn script_ref_parts(script_ref: &MultiEraScriptRef) -> ScriptRefParts {
    let language = script_ref.language();

    let bytes = match language {
        ScriptLanguage::Native => script_ref.native_script().map(|script| script.encode()),
        ScriptLanguage::PlutusV1
        | ScriptLanguage::PlutusV2
        | ScriptLanguage::PlutusV3
        | ScriptLanguage::PlutusV4 => script_ref.plutus_bytes().map(|bytes| bytes.to_vec()),
        other => panic!("script_ref_parts has no bytes rule for {other:?}"),
    };

    ScriptRefParts {
        language,
        hash: script_ref.hash(),
        // Reachable only if the language a reference script reports and the
        // bytes it returns disagree. No variant of ScriptRefParts can state
        // that, and serving a script with the wrong bytes under the right hash
        // is worse than stopping.
        bytes: bytes.expect("a reference script reporting a language carries that language's bytes"),
    }
}

/// Compute the on-chain script hash of a reference script.
pub fn script_ref_hash(script_ref: &MultiEraScriptRef) -> Hash<28> {
    script_ref.hash()
}

pub const DREP_KEY_PREFIX: u8 = 0b00100010;
pub const DREP_SCRIPT_PREFIX: u8 = 0b00100011;

/// Check that the first byte of the drep id finishes with the 0011 bytes.
pub fn drep_id_is_script(drep_id: &[u8]) -> bool {
    let first = drep_id.first().unwrap();
    first & 0b00001111 == 0b00000011
}

pub fn stake_cred_to_drep(cred: &StakeCredential) -> DRep {
    match cred {
        StakeCredential::AddrKeyhash(key) => DRep::Key(*key),
        StakeCredential::ScriptHash(key) => DRep::Script(*key),
    }
}

pub fn parse_reward_account(reward_account: &[u8]) -> Option<StakeCredential> {
    let pool_address = Address::from_bytes(reward_account).ok()?;
    let (cred, _) = address_as_stake_cred(&pool_address)?;

    Some(cred)
}

pub fn keyhash_to_stake_cred(keyhash: Hash<28>) -> StakeCredential {
    StakeCredential::AddrKeyhash(keyhash)
}

pub fn cred_matches_hash(cred: &StakeCredential, hash: &str) -> bool {
    let hash: Hash<28> = hash.parse().unwrap();

    match cred {
        StakeCredential::AddrKeyhash(x) => x == &hash,
        StakeCredential::ScriptHash(x) => x == &hash,
    }
}

pub fn tx_treasury_donation(tx: &MultiEraTx) -> Option<Lovelace> {
    match tx {
        MultiEraTx::Conway(x) => x.transaction_body.donation.map(|x| x.into()),
        // Dijkstra keeps the donation at the same body key as Conway. An era
        // that carries the field has to read it, because the wildcard below
        // stops the node rather than answering, and a chain past the Dijkstra
        // hard fork puts every one of its transactions through here.
        MultiEraTx::Dijkstra(x) => x.transaction_body.donation.map(|x| x.into()),
        MultiEraTx::AlonzoCompatible(..) => None,
        MultiEraTx::Babbage(..) => None,
        MultiEraTx::Byron(..) => None,
        _ => panic!("unexpected tx era"),
    }
}

#[cfg(test)]
mod script_ref_tests {
    use super::*;
    use pallas::crypto::hash::Hasher;
    use pallas::ledger::traverse::Era;

    /// The three bytes of a minimal Plutus script, the same ones pallas uses
    /// in its own reference script tests.
    const PLUTUS_BYTES: [u8; 3] = [0x4d, 0x01, 0x00];

    /// `native_script = [0, addr_keyhash]`, the pubkey clause over 28 bytes.
    fn native_script() -> Vec<u8> {
        let mut out = vec![0x82, 0x00, 0x58, 0x1c];
        out.extend_from_slice(&[0xaa; 28]);
        out
    }

    /// `script = [n, script_n]`, the reference script rule of Babbage on.
    fn script(tag: u8, inner: &[u8]) -> Vec<u8> {
        let mut out = vec![0x82, tag];
        out.extend_from_slice(inner);
        out
    }

    /// The hash the ledger keys a script by, computed from the rule rather
    /// than from the code under test: blake2b-224 over the language tag
    /// followed by the script's own bytes.
    fn ledger_hash(tag: u8, bytes: &[u8]) -> Hash<28> {
        let mut input = vec![tag];
        input.extend_from_slice(bytes);
        Hasher::<224>::hash(&input)
    }

    #[test]
    fn a_dijkstra_plutus_v4_reference_script_reports_its_language_hash_and_bytes() {
        let cbor = script(4, &[0x43, 0x4d, 0x01, 0x00]);
        let script_ref = MultiEraScriptRef::decode(Era::Dijkstra, &cbor).expect("must decode");

        let parts = script_ref_parts(&script_ref);

        assert_eq!(parts.language, ScriptLanguage::PlutusV4);
        assert_eq!(parts.bytes, PLUTUS_BYTES.to_vec());
        assert_eq!(parts.hash, ledger_hash(4, &PLUTUS_BYTES));
        // The tag is what separates one language's hash from another's over
        // the same bytes, so a hash taken without it is the wrong hash.
        assert_ne!(parts.hash, Hasher::<224>::hash(&PLUTUS_BYTES));
        assert_ne!(parts.hash, ledger_hash(3, &PLUTUS_BYTES));
        // The whole tagged array is what a reference script is stored as, so
        // it has to come back with the tag on it and not as the body alone.
        assert_eq!(script_ref.encode(), cbor);
    }

    #[test]
    fn a_dijkstra_native_reference_script_reports_its_language_hash_and_bytes() {
        let inner = native_script();
        let cbor = script(0, &inner);
        let script_ref = MultiEraScriptRef::decode(Era::Dijkstra, &cbor).expect("must decode");

        let parts = script_ref_parts(&script_ref);

        assert_eq!(parts.language, ScriptLanguage::Native);
        assert_eq!(parts.bytes, inner);
        assert_eq!(parts.hash, ledger_hash(0, &inner));
    }

    #[test]
    fn a_conway_native_reference_script_reports_its_language_hash_and_bytes() {
        let inner = native_script();
        let cbor = script(0, &inner);
        let script_ref = MultiEraScriptRef::decode(Era::Conway, &cbor).expect("must decode");

        let parts = script_ref_parts(&script_ref);

        assert_eq!(parts.language, ScriptLanguage::Native);
        assert_eq!(parts.bytes, inner);
        assert_eq!(parts.hash, ledger_hash(0, &inner));
        assert_eq!(script_ref_hash(&script_ref), parts.hash);
        assert_eq!(script_ref.encode(), cbor);
    }

    #[test]
    fn a_conway_plutus_v2_reference_script_reports_its_language_hash_and_bytes() {
        let cbor = script(2, &[0x43, 0x4d, 0x01, 0x00]);
        let script_ref = MultiEraScriptRef::decode(Era::Conway, &cbor).expect("must decode");

        let parts = script_ref_parts(&script_ref);

        assert_eq!(parts.language, ScriptLanguage::PlutusV2);
        assert_eq!(parts.bytes, PLUTUS_BYTES.to_vec());
        assert_eq!(parts.hash, ledger_hash(2, &PLUTUS_BYTES));
    }
}

#[cfg(test)]
mod dijkstra_certificate_tests {
    use super::*;
    use pallas::ledger::primitives::dijkstra::Certificate as DijkstraCert;
    use std::borrow::Cow;

    const POOL: &str = "747aca09f322d2dfc56243b839e2d573ab92287684e5e37d66ec0f87";
    const VRF: &str = "d8252bd637a90ba4dbd2cf63afda20a19888b7895ede067081ce7fb7411a972b";
    const CRED: &str = "5e81366cb6f3c0d14837614afcea669d51b8be9519eaec4a237504f8";

    fn wrap(cert: DijkstraCert) -> MultiEraCert<'static> {
        MultiEraCert::Dijkstra(Box::new(Cow::Owned(cert)))
    }

    fn cred() -> StakeCredential {
        StakeCredential::AddrKeyhash(CRED.parse().unwrap())
    }

    fn pool_registration() -> DijkstraCert {
        DijkstraCert::PoolRegistration {
            operator: POOL.parse().unwrap(),
            vrf_keyhash: VRF.parse().unwrap(),
            // The BLS key slot is what makes a Dijkstra pool registration a
            // different shape from Conway's. Present and populated is the
            // interesting one of its three states.
            bls_key: Some(pallas::codec::utils::Nullable::Null),
            pledge: 1_000_000,
            cost: 340_000_000,
            margin: RationalNumber {
                numerator: 3,
                denominator: 100,
            },
            reward_account: vec![0xe0].into(),
            pool_owners: vec![CRED.parse::<Hash<28>>().unwrap()].into(),
            relays: vec![],
            pool_metadata: None,
        }
    }

    /// Which of the eleven accessors answers for a given certificate. Any
    /// accessor that answers is named, so a test can say both which one fired
    /// and that no other one did.
    fn answered_by(cert: &MultiEraCert) -> Vec<&'static str> {
        let mut out = vec![];

        if cert_as_pool_registration(cert).is_some() {
            out.push("pool_registration");
        }
        if cert_as_pool_retirement(cert).is_some() {
            out.push("pool_retirement");
        }
        if cert_as_vote_delegation(cert).is_some() {
            out.push("vote_delegation");
        }
        if cert_as_drep_registration(cert).is_some() {
            out.push("drep_registration");
        }
        if cert_as_drep_unregistration(cert).is_some() {
            out.push("drep_unregistration");
        }
        if cert_as_committee_auth(cert).is_some() {
            out.push("committee_auth");
        }
        if cert_as_committee_resign(cert).is_some() {
            out.push("committee_resign");
        }
        if cert_as_stake_delegation(cert).is_some() {
            out.push("stake_delegation");
        }
        if cert_as_stake_registration(cert).is_some() {
            out.push("stake_registration");
        }
        if cert_as_stake_deregistration(cert).is_some() {
            out.push("stake_deregistration");
        }
        if cert_as_mir_certificate(cert).is_some() {
            out.push("mir");
        }

        out
    }

    /// The must-fire case, over every certificate a Dijkstra chain can carry.
    /// Musashi is Dijkstra from slot 86400, so every certificate the node ever
    /// applies past that point arrives through this type.
    #[test]
    fn every_dijkstra_certificate_kind_is_read() {
        let cases: Vec<(DijkstraCert, &str)> = vec![
            (pool_registration(), "pool_registration"),
            (
                DijkstraCert::PoolRetirement(POOL.parse().unwrap(), 42),
                "pool_retirement",
            ),
            (DijkstraCert::Reg(cred(), 2_000_000), "stake_registration"),
            (DijkstraCert::UnReg(cred(), 2_000_000), "stake_deregistration"),
            (
                DijkstraCert::StakeDelegation(cred(), POOL.parse().unwrap()),
                "stake_delegation",
            ),
            (
                DijkstraCert::VoteDeleg(cred(), DRep::Abstain),
                "vote_delegation",
            ),
            (
                DijkstraCert::RegDRepCert(cred(), 500_000_000, None),
                "drep_registration",
            ),
            (
                DijkstraCert::UnRegDRepCert(cred(), 500_000_000),
                "drep_unregistration",
            ),
            (
                DijkstraCert::AuthCommitteeHot(cred(), cred()),
                "committee_auth",
            ),
            (
                DijkstraCert::ResignCommitteeCold(cred(), None),
                "committee_resign",
            ),
        ];

        for (cert, expected) in cases {
            let wrapped = wrap(cert);
            let answered = answered_by(&wrapped);

            assert!(
                answered.contains(&expected),
                "{expected} did not answer for {wrapped:?}, answered: {answered:?}",
            );
        }
    }

    /// The must-not case. Reading Dijkstra certificates must not turn every
    /// accessor into one that answers for everything, so a pool retirement has
    /// to be read as a pool retirement and as nothing else.
    #[test]
    fn a_dijkstra_certificate_answers_only_its_own_accessor() {
        let cert = wrap(DijkstraCert::PoolRetirement(POOL.parse().unwrap(), 42));

        assert_eq!(answered_by(&cert), vec!["pool_retirement"]);

        let cert = wrap(DijkstraCert::Reg(cred(), 2_000_000));

        assert_eq!(answered_by(&cert), vec!["stake_registration"]);

        // A registration that also delegates is genuinely two things, and both
        // accessors are meant to answer.
        let cert = wrap(DijkstraCert::StakeVoteRegDeleg(
            cred(),
            POOL.parse().unwrap(),
            DRep::Abstain,
            2_000_000,
        ));

        let answered = answered_by(&cert);

        assert!(answered.contains(&"vote_delegation"));
        assert!(answered.contains(&"stake_delegation"));
        assert!(!answered.contains(&"pool_retirement"));
        assert!(!answered.contains(&"committee_auth"));
    }

    /// The pool parameters have to survive the read, not merely be present.
    /// An accessor that answered with a default would pass the test above.
    #[test]
    fn a_dijkstra_pool_registration_carries_its_parameters() {
        let cert = wrap(pool_registration());

        let read = cert_as_pool_registration(&cert).expect("must be read");

        assert_eq!(read.operator, POOL.parse::<Hash<28>>().unwrap());
        assert_eq!(read.vrf_keyhash, VRF.parse::<Hash<32>>().unwrap());
        assert_eq!(read.pledge, 1_000_000);
        assert_eq!(read.cost, 340_000_000);
        assert_eq!(read.margin.numerator, 3);
        assert_eq!(read.margin.denominator, 100);
        assert_eq!(read.reward_account, vec![0xe0]);
        assert_eq!(read.pool_owners, vec![CRED.parse::<Hash<28>>().unwrap()]);
    }
}

#[cfg(test)]
mod treasury_donation_tests {
    use super::*;

    /// The smallest Dijkstra transaction that carries a treasury donation:
    /// one input, no outputs, zero fee, and body key 22 set to 1000000. Built
    /// by hand because no transaction on any Dijkstra chain has ever set that
    /// key, so there are no real bytes to take it from.
    ///
    /// Four elements, ending `f5`. The w36 ledger deleted the block body's
    /// leading list of transactions the producer rejected and put each
    /// producer's verdict on the transaction instead, so a transaction as a
    /// block carries it is `[body, witness_set, auxiliary_data / nil, bool]`
    /// and `decode_for_era` reads that shape. Three elements is the mempool
    /// form, which is what a client submits and not what a block holds.
    const DIJKSTRA_TX_WITH_DONATION: &str = "84a40081825820000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f0001800200161a000f4240a0f6f5";

    /// The same transaction with body key 22 absent.
    const DIJKSTRA_TX_WITHOUT_DONATION: &str = "84a30081825820000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f0001800200a0f6f5";

    fn dijkstra_tx(hex: &str) -> MultiEraTx<'static> {
        let bytes: &'static [u8] = hex::decode(hex).unwrap().leak();
        MultiEraTx::decode_for_era(pallas::ledger::traverse::Era::Dijkstra, bytes).unwrap()
    }

    /// The must-fire case. A Dijkstra transaction's donation has to come back
    /// as the amount it carries. Musashi is a Dijkstra chain from slot 86400
    /// on, so every transaction the node applies reaches this accessor.
    #[test]
    fn a_dijkstra_donation_is_read() {
        let tx = dijkstra_tx(DIJKSTRA_TX_WITH_DONATION);
        assert!(matches!(tx, MultiEraTx::Dijkstra(..)));
        assert_eq!(tx_treasury_donation(&tx), Some(1_000_000));
    }

    /// The must-not case. Reading the field must not turn every Dijkstra
    /// transaction into a donation, so one without the key has to come back
    /// as none.
    #[test]
    fn a_dijkstra_transaction_without_a_donation_reports_none() {
        let tx = dijkstra_tx(DIJKSTRA_TX_WITHOUT_DONATION);
        assert!(matches!(tx, MultiEraTx::Dijkstra(..)));
        assert_eq!(tx_treasury_donation(&tx), None);
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::model::pools::testing::any_pool_params;
    use crate::model::testing as root;
    use proptest::prelude::*;

    prop_compose! {
        pub fn any_multi_era_pool_registration()(
            operator in root::any_hash_28(),
            params in any_pool_params(),
        ) -> MultiEraPoolRegistration {
            MultiEraPoolRegistration {
                operator,
                vrf_keyhash: params.vrf_keyhash,
                pledge: params.pledge,
                cost: params.cost,
                margin: params.margin,
                reward_account: params.reward_account,
                pool_owners: params.pool_owners,
                relays: params.relays,
                pool_metadata: params.pool_metadata,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static REWARD_ACCOUNT: [u8; 29] = [
        224, 185, 111, 206, 243, 185, 53, 26, 246, 131, 75, 216, 80, 227, 169, 120, 89, 215, 189,
        91, 114, 157, 36, 191, 54, 70, 174, 172, 207,
    ];

    #[test]
    fn test_pool_reward_account() {
        let parsed = parse_reward_account(&REWARD_ACCOUNT).unwrap();
        dbg!(&parsed);
    }
}
