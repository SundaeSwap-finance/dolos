//! The pools, DReps, committee, constitution and proposals the ledger holds
//! after a block, against those Dolos holds.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use crate::{
    accounts::Credential,
    compare::Tally,
    judgement::{Axis, CellKey, EntryPoint, Finding, Instrument},
    record::{PParams, RecordId},
    state::{
        BlsKey, BlsKeyPair, Certs, Committee, CommitteeState, Constitution, DRepState, Gov,
        GovAction, NotHeld, PoolParams, Pools, Proposal, ProposalProcedure, Roots, StakePool, Vote,
        Votes,
    },
};

/// A parameter update as the debug form of each value it sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamUpdate(pub BTreeSet<String>);

/// A field value Dolos holds that the ledger's types cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unrepresentable {
    pub table: &'static str,
    pub key: String,
    pub field: &'static str,
    pub dolos: String,
}

/// The governance state Dolos holds, in the ledger's types.
#[derive(Debug, Clone, PartialEq)]
pub struct DolosGov {
    pub committee: Option<Committee>,
    pub constitution: Option<Constitution>,
    pub proposals: Vec<Proposal<NotHeld, ParamUpdate>>,
    pub roots: Roots,
}

/// The pools, DReps, committee, constitution and proposals Dolos holds, in the
/// ledger's types.
#[derive(Debug, Clone, PartialEq)]
pub struct DolosState {
    pub certs: Certs<NotHeld>,
    pub gov: DolosGov,
    pub unrepresentable: Vec<Unrepresentable>,
}

/// A ledger state table and the name of each field a row of it holds.
pub struct Table<const N: usize> {
    pub name: &'static str,
    pub fields: [&'static str; N],
}

const STAKE_POOLS: Table<11> = Table {
    name: "stake_pools",
    fields: [
        "account_id",
        "bls_key",
        "cost",
        "delegators",
        "deposit",
        "margin",
        "metadata",
        "owners",
        "pledge",
        "relays",
        "vrf",
    ],
};

const FUTURE_STAKE_POOL_PARAMS: Table<10> = Table {
    name: "future_stake_pool_params",
    fields: [
        "account_address",
        "bls_key",
        "cost",
        "margin",
        "metadata",
        "owners",
        "pledge",
        "pool_id",
        "relays",
        "vrf",
    ],
};

const RETIRING: Table<1> = Table {
    name: "retiring",
    fields: ["epoch"],
};

const VRF_KEY_HASHES: Table<1> = Table {
    name: "vrf_key_hashes",
    fields: ["count"],
};

const DREPS: Table<3> = Table {
    name: "dreps",
    fields: ["anchor", "deposit", "expiry"],
};

const COMMITTEE_CREDS: Table<1> = Table {
    name: "committee_creds",
    fields: ["authorization"],
};

const DORMANT_EPOCHS: Table<1> = Table {
    name: "dormant_epochs",
    fields: ["count"],
};

const COMMITTEE: Table<2> = Table {
    name: "committee",
    fields: ["members", "threshold"],
};

const CONSTITUTION: Table<2> = Table {
    name: "constitution",
    fields: ["anchor", "script"],
};

const ROOTS: Table<4> = Table {
    name: "roots",
    fields: ["committee", "constitution", "hard_fork", "pparam_update"],
};

const PROPOSALS: Table<9> = Table {
    name: "proposals",
    fields: [
        "anchor",
        "committee_votes",
        "deposit",
        "drep_votes",
        "expires_after",
        "gov_action",
        "proposed_in",
        "return_addr",
        "stake_pool_votes",
    ],
};

/// The fields of each table the judge compares, by table.
const TABLES: [(&str, &[&str]); 11] = [
    (STAKE_POOLS.name, &STAKE_POOLS.fields),
    (
        FUTURE_STAKE_POOL_PARAMS.name,
        &FUTURE_STAKE_POOL_PARAMS.fields,
    ),
    (RETIRING.name, &RETIRING.fields),
    (VRF_KEY_HASHES.name, &VRF_KEY_HASHES.fields),
    (DREPS.name, &DREPS.fields),
    (COMMITTEE_CREDS.name, &COMMITTEE_CREDS.fields),
    (DORMANT_EPOCHS.name, &DORMANT_EPOCHS.fields),
    (COMMITTEE.name, &COMMITTEE.fields),
    (CONSTITUTION.name, &CONSTITUTION.fields),
    (ROOTS.name, &ROOTS.fields),
    (PROPOSALS.name, &PROPOSALS.fields),
];

/// The ledger state fields Dolos does not hold, by table.
pub const NOT_HELD: [Leaf; 3] = [
    Leaf {
        table: "stake_pools",
        field: "bls_registered_in",
    },
    Leaf {
        table: "proposals",
        field: "order",
    },
    Leaf {
        table: "proposals",
        field: "policy",
    },
];

/// A field of a ledger state table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Leaf {
    pub table: &'static str,
    pub field: &'static str,
}

impl fmt::Display for Leaf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.table, self.field)
    }
}

/// Every field of every table the judge compares.
pub fn every_leaf() -> Vec<Leaf> {
    let fields = TABLES
        .iter()
        .flat_map(|(table, fields)| fields.iter().map(move |field| Leaf { table, field }));
    fields.collect()
}

/// The rows of one table by key, each the shown value of every field.
type Rows<const N: usize> = BTreeMap<String, [String; N]>;

/// The rows of every table one side holds.
struct Tables {
    stake_pools: Rows<11>,
    future_stake_pool_params: Rows<10>,
    retiring: Rows<1>,
    vrf_key_hashes: Rows<1>,
    dreps: Rows<3>,
    committee_creds: Rows<1>,
    dormant_epochs: Rows<1>,
    committee: Rows<2>,
    constitution: Rows<2>,
    roots: Rows<4>,
    proposals: Rows<9>,
}

/// The key of a table that holds one row.
const SINGLE: &str = "state";

fn show<T: fmt::Debug>(value: &T) -> String {
    format!("{value:?}")
}

/// The shown members of a set, sorted.
fn set<T: fmt::Display>(items: impl IntoIterator<Item = T>) -> String {
    let shown: BTreeSet<String> = items.into_iter().map(|i| i.to_string()).collect();
    show(&shown)
}

/// The shown entries of a map, sorted by shown key.
fn map<K: fmt::Display, V: fmt::Debug>(entries: &BTreeMap<K, V>) -> String {
    set(entries.iter().map(|(k, v)| format!("{k}:{v:?}")))
}

fn bls(key: &BlsKeyPair) -> String {
    let BlsKeyPair {
        bls_possession_proof,
        bls_pub_key,
    } = key;
    let (proof, public) = (&bls_possession_proof.0, &bls_pub_key.0);
    format!("{}/{}", hex::encode(public), hex::encode(proof))
}

fn single<const N: usize>(row: Option<[String; N]>) -> Rows<N> {
    row.map(|r| (SINGLE.to_string(), r)).into_iter().collect()
}

fn stake_pool<E>(pool: &StakePool<E>) -> [String; 11] {
    let StakePool {
        sps_account_id,
        sps_bls_key,
        sps_cost,
        sps_delegators,
        sps_deposit,
        sps_margin,
        sps_metadata,
        sps_owners,
        sps_pledge,
        sps_relays,
        sps_vrf,
    } = pool;
    let bls_key = sps_bls_key.as_ref().map(
        |BlsKey {
             bks_key,
             bks_registered_in: _,
         }| bls(bks_key),
    );
    let delegators = sps_delegators.iter().map(|c| Credential::from(*c));
    [
        show(sps_account_id),
        show(&bls_key),
        show(sps_cost),
        set(delegators),
        show(sps_deposit),
        show(sps_margin),
        show(sps_metadata),
        set(sps_owners),
        show(sps_pledge),
        show(sps_relays),
        show(sps_vrf),
    ]
}

fn future_pool(params: &PoolParams) -> [String; 10] {
    let PoolParams {
        account_address,
        bls_key,
        cost,
        margin,
        metadata,
        owners,
        pledge,
        pool_id,
        relays,
        vrf,
    } = params;
    [
        show(account_address),
        show(&bls_key.as_ref().map(bls)),
        show(cost),
        show(margin),
        show(metadata),
        set(owners),
        show(pledge),
        show(pool_id),
        show(relays),
        show(vrf),
    ]
}

fn drep(state: &DRepState) -> [String; 3] {
    let DRepState {
        anchor,
        deposit,
        expiry,
    } = state;
    [show(anchor), show(deposit), show(expiry)]
}

fn committee(committee: &Committee) -> [String; 2] {
    let Committee { members, threshold } = committee;
    [map(members), show(threshold)]
}

fn constitution(constitution: &Constitution) -> [String; 2] {
    let Constitution { anchor, script } = constitution;
    [show(anchor), show(script)]
}

fn roots(roots: &Roots) -> [String; 4] {
    let Roots {
        committee,
        constitution,
        hard_fork,
        pparam_update,
    } = roots;
    [
        show(committee),
        show(constitution),
        show(hard_fork),
        show(pparam_update),
    ]
}

fn votes<K: fmt::Display>(votes: &BTreeMap<K, Vote>) -> String {
    map(votes)
}

/// The action without its guardrail policy, with its parameter update as
/// `update` writes it.
fn action<P, U>(action: &GovAction<P, U>, update: &dyn Fn(&U) -> ParamUpdate) -> String {
    match action {
        GovAction::ParameterChange(prev, u, _) => {
            format!("ParameterChange({prev:?}, {:?})", update(u).0)
        }
        GovAction::HardForkInitiation(prev, version) => {
            format!("HardForkInitiation({prev:?}, {version:?})")
        }
        GovAction::TreasuryWithdrawals(withdrawals, _) => {
            let paid = withdrawals.iter().map(|(a, coin)| format!("{a:?}:{coin}"));
            format!("TreasuryWithdrawals({})", set(paid))
        }
        GovAction::NoConfidence(prev) => format!("NoConfidence({prev:?})"),
        GovAction::UpdateCommittee(prev, removed, added, threshold) => {
            let removed = removed.iter().map(|c| Credential::from(*c));
            let (removed, added) = (set(removed), map(added));
            format!("UpdateCommittee({prev:?}, {removed}, {added}, {threshold:?})")
        }
        GovAction::NewConstitution(prev, c) => format!("NewConstitution({prev:?}, {c:?})"),
        GovAction::InfoAction => "InfoAction".to_string(),
    }
}

fn proposal<P, U>(proposal: &Proposal<P, U>, update: &dyn Fn(&U) -> ParamUpdate) -> [String; 9] {
    let Proposal {
        action_id: _,
        committee_votes,
        drep_votes,
        expires_after,
        proposal_procedure,
        proposed_in,
        stake_pool_votes,
    } = proposal;
    let ProposalProcedure {
        anchor,
        deposit,
        gov_action,
        return_addr,
    } = proposal_procedure;
    [
        show(anchor),
        votes(committee_votes),
        show(deposit),
        votes(drep_votes),
        show(expires_after),
        action(gov_action, update),
        show(proposed_in),
        show(return_addr),
        votes(stake_pool_votes),
    ]
}

/// The rows of the pool and vote tables of certificate state.
fn certs_tables<E>(
    certs: &Certs<E>,
) -> (
    Rows<11>,
    Rows<10>,
    Rows<1>,
    Rows<1>,
    Rows<3>,
    Rows<1>,
    Rows<1>,
) {
    let Certs {
        pstate:
            Pools {
                future_stake_pool_params,
                retiring,
                stake_pools,
                vrf_key_hashes,
            },
        vstate:
            Votes {
                committee_state: CommitteeState { cs_committee_creds },
                dreps,
                num_dormant_epochs,
            },
    } = certs;
    let pools = stake_pools
        .iter()
        .map(|(id, p)| (id.to_string(), stake_pool(p)));
    let future = future_stake_pool_params.iter();
    let future = future.map(|(id, p)| (id.to_string(), future_pool(p)));
    let retiring = retiring
        .iter()
        .map(|(id, epoch)| (id.to_string(), [show(epoch)]));
    let vrf = vrf_key_hashes
        .iter()
        .map(|(vrf, n)| (vrf.to_string(), [show(n)]));
    let dreps = dreps.iter().map(|(c, d)| (c.to_string(), drep(d)));
    let creds = cs_committee_creds
        .iter()
        .map(|(c, a)| (c.to_string(), [show(a)]));
    (
        pools.collect(),
        future.collect(),
        retiring.collect(),
        vrf.collect(),
        dreps.collect(),
        creds.collect(),
        single(Some([show(num_dormant_epochs)])),
    )
}

impl Tables {
    fn of<E, P, U>(
        certs: &Certs<E>,
        committee: Option<&Committee>,
        constitution: Option<&Constitution>,
        proposals: &[Proposal<P, U>],
        roots: &Roots,
        update: &dyn Fn(&U) -> ParamUpdate,
    ) -> Tables {
        let (
            stake_pools,
            future_stake_pool_params,
            retiring,
            vrf_key_hashes,
            dreps,
            committee_creds,
            dormant_epochs,
        ) = certs_tables(certs);
        let proposals = proposals.iter().map(|p| {
            let id = p.action_id;
            let key = format!("{}#{}", id.tx_id, id.gov_action_ix);
            (key, proposal(p, update))
        });
        Tables {
            stake_pools,
            future_stake_pool_params,
            retiring,
            vrf_key_hashes,
            dreps,
            committee_creds,
            dormant_epochs,
            committee: single(committee.map(self::committee)),
            constitution: single(constitution.map(self::constitution)),
            roots: single(Some(self::roots(roots))),
            proposals: proposals.collect(),
        }
    }
}

/// The ledger state comparisons of an arm and the fields they compared.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LedgerStateChecks {
    pub tally: Tally,
    pub compared: BTreeSet<Leaf>,
}

impl LedgerStateChecks {
    /// The entry point that answers the ledger state comparisons.
    pub const ENTRY: EntryPoint = EntryPoint::Instrument(Instrument::ReadLedgerState);

    /// Counts each row either side holds of one table against the other
    /// side's row with the same key.
    fn table<const N: usize>(
        &mut self,
        id: RecordId,
        table: &Table<N>,
        mut ledger: Rows<N>,
        mut dolos: Rows<N>,
    ) {
        let name = table.name;
        let keys: BTreeSet<String> = ledger.keys().chain(dolos.keys()).cloned().collect();
        for key in keys {
            let (l, d) = (ledger.remove(&key), dolos.remove(&key));
            let one_side = |answer: &str| {
                let cells = vec![
                    CellKey::new(Axis::Answer, answer),
                    CellKey::new(Axis::Table, name),
                ];
                Finding::new(id, cells, format!("table={name} key={key} {answer}"))
            };
            let (l, d) = match (l, d) {
                (Some(l), Some(d)) => (l, d),
                (Some(_), None) => {
                    self.tally.add(false, || one_side("ledger_only"));
                    continue;
                }
                (None, _) => {
                    self.tally.add(false, || one_side("dolos_only"));
                    continue;
                }
            };
            let fields = table.fields.iter().zip(l.iter().zip(&d));
            let differing: Vec<_> = fields.filter(|(_, (l, d))| l != d).collect();
            let leaves = table.fields.map(|field| Leaf { table: name, field });
            self.compared.extend(leaves);
            self.tally.add(differing.is_empty(), || {
                let shown = differing
                    .iter()
                    .map(|(f, (l, d))| format!("{f}: ledger={l} dolos={d}"));
                let cells = differing.iter().map(|(f, _)| CellKey::new(Axis::Field, f));
                let head = [
                    CellKey::new(Axis::Answer, "differ"),
                    CellKey::new(Axis::Table, name),
                ];
                let detail = format!(
                    "table={name} key={key} {}",
                    shown.collect::<Vec<_>>().join(" ")
                );
                Finding::new(id, head.into_iter().chain(cells).collect(), detail)
            });
        }
    }

    /// Adds the comparisons and compared fields of another arm part.
    pub fn merge(&mut self, other: LedgerStateChecks) {
        let LedgerStateChecks { tally, compared } = other;
        self.tally.merge(tally);
        self.compared.extend(compared);
    }

    /// The line that says how many fields of the held tables a comparison
    /// reached, the fields none reached and the fields Dolos does not hold.
    pub fn deep_line(&self) -> String {
        let every = every_leaf();
        let uncompared: Vec<String> = every
            .iter()
            .filter(|leaf| !self.compared.contains(leaf))
            .map(Leaf::to_string)
            .collect();
        let not_held: Vec<String> = NOT_HELD.iter().map(Leaf::to_string).collect();
        format!(
            "DEEP judge=ledger-state fields_compared={} fields_total={} disagree={} uncompared={} not_held={}",
            every.len() - uncompared.len(),
            every.len(),
            self.tally.disagree.len(),
            uncompared.join(","),
            not_held.join(",")
        )
    }
}

/// Counts each row of every ledger state table either side holds after record
/// `id` against the other side's row, with each parameter update of the
/// ledger as `update` writes it, and each value Dolos holds that the ledger's
/// types cannot.
pub fn compare(
    id: RecordId,
    ledger: (&Certs, &Gov),
    dolos: &DolosState,
    update: &dyn Fn(&PParams) -> ParamUpdate,
    checks: &mut LedgerStateChecks,
) {
    let (certs, gov) = ledger;
    let Gov {
        committee,
        constitution,
        proposals,
        roots,
    } = gov;
    let ledger = Tables::of(
        certs,
        committee.as_ref(),
        Some(constitution),
        proposals,
        roots,
        update,
    );
    let DolosState {
        certs,
        gov,
        unrepresentable,
    } = dolos;
    let DolosGov {
        committee,
        constitution,
        proposals,
        roots,
    } = gov;
    let dolos = Tables::of(
        certs,
        committee.as_ref(),
        constitution.as_ref(),
        proposals,
        roots,
        &|u: &ParamUpdate| u.clone(),
    );
    let Tables {
        stake_pools,
        future_stake_pool_params,
        retiring,
        vrf_key_hashes,
        dreps,
        committee_creds,
        dormant_epochs,
        committee,
        constitution,
        roots,
        proposals,
    } = ledger;
    checks.table(id, &STAKE_POOLS, stake_pools, dolos.stake_pools);
    let future = dolos.future_stake_pool_params;
    checks.table(
        id,
        &FUTURE_STAKE_POOL_PARAMS,
        future_stake_pool_params,
        future,
    );
    checks.table(id, &RETIRING, retiring, dolos.retiring);
    checks.table(id, &VRF_KEY_HASHES, vrf_key_hashes, dolos.vrf_key_hashes);
    checks.table(id, &DREPS, dreps, dolos.dreps);
    checks.table(id, &COMMITTEE_CREDS, committee_creds, dolos.committee_creds);
    checks.table(id, &DORMANT_EPOCHS, dormant_epochs, dolos.dormant_epochs);
    checks.table(id, &COMMITTEE, committee, dolos.committee);
    checks.table(id, &CONSTITUTION, constitution, dolos.constitution);
    checks.table(id, &ROOTS, roots, dolos.roots);
    checks.table(id, &PROPOSALS, proposals, dolos.proposals);
    for Unrepresentable {
        table,
        key,
        field,
        dolos,
    } in unrepresentable
    {
        let cells = vec![
            CellKey::new(Axis::Answer, "differ"),
            CellKey::new(Axis::Table, table),
            CellKey::new(Axis::Field, field),
        ];
        let detail = format!("table={table} key={key} {field}: ledger=none dolos={dolos}");
        checks.tally.add(false, || Finding::new(id, cells, detail));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        record::{Bytes, CorpusKind},
        state::{
            ActionId, Anchor, CommitteeAuthorization, CredentialObject, Fraction, Hex,
            RewardAccount,
        },
    };

    fn id() -> RecordId {
        RecordId {
            corpus: CorpusKind::Ic,
            n: 7,
        }
    }

    fn hash<const N: usize>(b: u8) -> Hex<N> {
        Hex([b; N])
    }

    fn anchor(b: u8) -> Anchor {
        Anchor {
            data_hash: hash(b),
            url: format!("https://example.com/{b}"),
        }
    }

    fn account(b: u8) -> RewardAccount {
        RewardAccount {
            credential: CredentialObject::KeyHash(hash(b)),
            network: "Testnet".to_string(),
        }
    }

    fn half() -> Fraction {
        Fraction {
            numerator: 1,
            denominator: 2,
        }
    }

    fn bls_pair() -> BlsKeyPair {
        BlsKeyPair {
            bls_possession_proof: Bytes(vec![1; 48]),
            bls_pub_key: Bytes(vec![2; 96]),
        }
    }

    fn pool<E>(registered_in: E) -> StakePool<E> {
        StakePool {
            sps_account_id: CredentialObject::KeyHash(hash(3)),
            sps_bls_key: Some(BlsKey {
                bks_key: bls_pair(),
                bks_registered_in: registered_in,
            }),
            sps_cost: 340,
            sps_delegators: vec![
                CredentialObject::ScriptHash(hash(4)),
                CredentialObject::KeyHash(hash(5)),
            ],
            sps_deposit: 500,
            sps_margin: half(),
            sps_metadata: None,
            sps_owners: vec![hash(6), hash(7)],
            sps_pledge: 100,
            sps_relays: Vec::new(),
            sps_vrf: hash(8),
        }
    }

    fn certs<E>(registered_in: E) -> Certs<E> {
        let future = PoolParams {
            account_address: account(3),
            bls_key: Some(bls_pair()),
            cost: 341,
            margin: half(),
            metadata: None,
            owners: vec![hash(6)],
            pledge: 101,
            pool_id: hash(9),
            relays: Vec::new(),
            vrf: hash(10),
        };
        let cold = Credential::Key([11; 28]);
        let hot =
            CommitteeAuthorization::CommitteeHotCredential(CredentialObject::KeyHash(hash(12)));
        let drep = DRepState {
            anchor: Some(anchor(13)),
            deposit: 2,
            expiry: 40,
        };
        Certs {
            pstate: Pools {
                future_stake_pool_params: BTreeMap::from([(hash(9), future)]),
                retiring: BTreeMap::from([(hash(9), 30)]),
                stake_pools: BTreeMap::from([(hash(9), pool(registered_in))]),
                vrf_key_hashes: BTreeMap::from([(hash(8), 1), (hash(10), 1)]),
            },
            vstate: Votes {
                committee_state: CommitteeState {
                    cs_committee_creds: BTreeMap::from([(cold, hot)]),
                },
                dreps: BTreeMap::from([(Credential::Script([14; 28]), drep)]),
                num_dormant_epochs: 3,
            },
        }
    }

    fn action_id(b: u8, ix: u32) -> ActionId {
        ActionId {
            gov_action_ix: ix,
            tx_id: hash(b),
        }
    }

    fn proposal<P, U>(update: U, policy: P) -> Proposal<P, U> {
        Proposal {
            action_id: action_id(20, 0),
            committee_votes: BTreeMap::from([(Credential::Key([12; 28]), Vote::VoteYes)]),
            drep_votes: BTreeMap::from([(Credential::Script([14; 28]), Vote::VoteNo)]),
            expires_after: 33,
            proposal_procedure: ProposalProcedure {
                anchor: anchor(21),
                deposit: 1000,
                gov_action: GovAction::ParameterChange(Some(action_id(22, 1)), update, policy),
                return_addr: account(23),
            },
            proposed_in: 27,
            stake_pool_votes: BTreeMap::from([(hash(9), Vote::Abstain)]),
        }
    }

    fn the_roots() -> Roots {
        Roots {
            committee: Some(action_id(24, 0)),
            constitution: None,
            hard_fork: None,
            pparam_update: Some(action_id(22, 1)),
        }
    }

    fn the_committee() -> Committee {
        Committee {
            members: BTreeMap::from([(Credential::Key([11; 28]), 50)]),
            threshold: half(),
        }
    }

    fn the_constitution() -> Constitution {
        Constitution {
            anchor: anchor(25),
            script: Some(hash(26)),
        }
    }

    fn params() -> PParams {
        let mut map = serde_json::Map::new();
        map.insert("maxTxSize".to_string(), serde_json::json!(16384));
        PParams(map)
    }

    fn written(p: &PParams) -> ParamUpdate {
        ParamUpdate(p.0.iter().map(|(k, v)| format!("{k}={v}")).collect())
    }

    fn ledger_gov() -> Gov {
        Gov {
            committee: Some(the_committee()),
            constitution: the_constitution(),
            proposals: vec![proposal(params(), Some(hash(26)))],
            roots: the_roots(),
        }
    }

    /// The Dolos state that holds every field of the ledger state it holds.
    fn agreeing() -> DolosState {
        DolosState {
            certs: certs(NotHeld),
            gov: DolosGov {
                committee: Some(the_committee()),
                constitution: Some(the_constitution()),
                proposals: vec![proposal(written(&params()), NotHeld)],
                roots: the_roots(),
            },
            unrepresentable: Vec::new(),
        }
    }

    fn judged(dolos: &DolosState) -> LedgerStateChecks {
        let mut checks = LedgerStateChecks::default();
        compare(
            id(),
            (&certs(4), &ledger_gov()),
            dolos,
            &written,
            &mut checks,
        );
        checks
    }

    fn findings(checks: &LedgerStateChecks) -> Vec<String> {
        checks
            .tally
            .disagree
            .iter()
            .map(Finding::to_string)
            .collect()
    }

    fn cells(checks: &LedgerStateChecks) -> Vec<Vec<String>> {
        let cells = checks
            .tally
            .disagree
            .iter()
            .map(|f| f.cells.iter().map(CellKey::to_string).collect());
        cells.collect()
    }

    #[test]
    fn an_agreeing_state_compares_every_held_field_and_disagrees_nowhere() {
        let checks = judged(&agreeing());
        assert_eq!(findings(&checks), Vec::<String>::new());
        assert_eq!(checks.tally.compared, 12);
        assert_eq!(checks.compared, every_leaf().into_iter().collect());
        assert_eq!(
            checks.deep_line(),
            "DEEP judge=ledger-state fields_compared=45 fields_total=45 disagree=0 uncompared= \
             not_held=stake_pools.bls_registered_in,proposals.order,proposals.policy"
        );
    }

    #[test]
    fn a_differing_pool_deposit_disagrees_at_that_field() {
        let mut dolos = agreeing();
        dolos
            .certs
            .pstate
            .stake_pools
            .get_mut(&hash(9))
            .unwrap()
            .sps_deposit = 0;
        let checks = judged(&dolos);
        assert_eq!(
            cells(&checks),
            vec![vec!["answer:differ", "table:stake_pools", "field:deposit"]]
        );
        let key = hash::<28>(9);
        assert_eq!(
            findings(&checks),
            vec![format!(
                "ic#7 table=stake_pools key={key} deposit: ledger=500 dolos=0"
            )]
        );
    }

    #[test]
    fn a_proposal_dolos_does_not_hold_is_ledger_only() {
        let mut dolos = agreeing();
        dolos.gov.proposals.clear();
        let checks = judged(&dolos);
        assert_eq!(
            cells(&checks),
            vec![vec!["answer:ledger_only", "table:proposals"]]
        );
        assert!(!checks.compared.contains(&Leaf {
            table: "proposals",
            field: "deposit"
        }));
    }

    #[test]
    fn a_drep_only_dolos_holds_is_dolos_only() {
        let mut dolos = agreeing();
        let drep = DRepState {
            anchor: None,
            deposit: 2,
            expiry: 41,
        };
        dolos
            .certs
            .vstate
            .dreps
            .insert(Credential::Key([15; 28]), drep);
        assert_eq!(
            cells(&judged(&dolos)),
            vec![vec!["answer:dolos_only", "table:dreps"]]
        );
    }

    #[test]
    fn a_differing_vote_or_parameter_update_disagrees_at_that_field() {
        let mut dolos = agreeing();
        let p = &mut dolos.gov.proposals[0];
        p.drep_votes
            .insert(Credential::Script([14; 28]), Vote::VoteYes);
        let GovAction::ParameterChange(_, update, _) = &mut p.proposal_procedure.gov_action else {
            unreachable!()
        };
        update.0.insert("maxTxSize=16385".to_string());
        assert_eq!(
            cells(&judged(&dolos)),
            vec![vec![
                "answer:differ",
                "table:proposals",
                "field:drep_votes",
                "field:gov_action"
            ]]
        );
    }

    #[test]
    fn a_missing_constitution_or_committee_is_ledger_only() {
        let mut dolos = agreeing();
        dolos.gov.constitution = None;
        dolos.gov.committee = None;
        assert_eq!(
            cells(&judged(&dolos)),
            vec![
                vec!["answer:ledger_only", "table:committee"],
                vec!["answer:ledger_only", "table:constitution"]
            ]
        );
    }

    #[test]
    fn a_value_the_ledger_types_cannot_hold_disagrees() {
        let mut dolos = agreeing();
        dolos.unrepresentable.push(Unrepresentable {
            table: "stake_pools",
            key: "09".to_string(),
            field: "relays",
            dolos: "1 relay".to_string(),
        });
        let checks = judged(&dolos);
        assert_eq!(
            cells(&checks),
            vec![vec!["answer:differ", "table:stake_pools", "field:relays"]]
        );
        assert_eq!(
            findings(&checks),
            vec!["ic#7 table=stake_pools key=09 relays: ledger=none dolos=1 relay"]
        );
    }

    #[test]
    fn merging_adds_the_comparisons_and_the_compared_fields() {
        let mut merged = LedgerStateChecks::default();
        let mut dolos = agreeing();
        dolos.gov.proposals.clear();
        merged.merge(judged(&dolos));
        assert!(merged.deep_line().contains(" uncompared=proposals.anchor,"));
        merged.merge(judged(&agreeing()));
        assert_eq!(merged.tally.compared, 24);
        assert_eq!(merged.tally.disagree.len(), 1);
        assert!(merged
            .deep_line()
            .contains(" fields_compared=45 fields_total=45 disagree=1 "));
    }
}
