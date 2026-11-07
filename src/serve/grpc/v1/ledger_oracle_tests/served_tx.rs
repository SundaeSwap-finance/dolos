//! Judges the fee, witness scripts and committee thresholds each ReadTx version maps for a transaction against the ledger's transaction.

use std::{fmt, panic::AssertUnwindSafe};

use dolos_cardano::indexes::ScriptLanguage;
use futures_util::FutureExt as _;
use itertools::Itertools as _;
use ledger_oracle_judge::{
    compare::Tally,
    eval::{Layer, WireFee},
    record::{CorpusKind, LedgerOutcome, RecordId},
};
use pallas::{
    crypto::hash::Hasher,
    ledger::traverse::{Era, MultiEraGovActionKind, MultiEraTx},
};
use tonic::Request;

use super::{eval::wire_fee, validate::panic_message, witness_scripts, GroupBlock, TxName};
use crate::adapters::DomainAdapter;

/// A witness script by its language, with its hash when the message carries its bytes, none for a script message with no script in it.
type ScriptKey = Option<(ScriptLanguage, Option<[u8; 28]>)>;

/// The threshold a proposal sets when it updates the committee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quorum {
    /// A proposal of another kind.
    Other,
    /// A committee update with this threshold, none when the message carries none.
    Update(Option<(i128, u128)>),
}

impl Quorum {
    /// Whether a mapped threshold is this one within 2^-30, which a pair of 32 bit fields holds for any threshold the ledger accepts.
    fn holds(self, mapped: Quorum) -> bool {
        match (self, mapped) {
            (Quorum::Other, Quorum::Other) => true,
            (Quorum::Update(Some((n, d))), Quorum::Update(Some((m, e)))) => {
                let gap = (n * e as i128 - m * d as i128).unsigned_abs();
                gap << 30 <= d * e
            }
            _ => false,
        }
    }
}

impl fmt::Display for Quorum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Quorum::Other => f.write_str("other"),
            Quorum::Update(Some((n, d))) => write!(f, "{n}/{d}"),
            Quorum::Update(None) => f.write_str("none"),
        }
    }
}

/// The fee, witness scripts and committee thresholds of a transaction as the ledger holds them.
struct LedgerTx {
    fee: Option<u64>,
    scripts: Vec<ScriptKey>,
    quorum: Vec<Quorum>,
}

impl LedgerTx {
    /// The fields the ledger holds for a transaction.
    fn of(tx: &MultiEraTx<'_>) -> Self {
        let scripts = witness_scripts(tx).into_iter().map(|(hash, language)| {
            let hash = (language != ScriptLanguage::Native).then_some(hash);
            Some((language, hash))
        });
        let proposals = tx.gov_proposals();
        let quorum = proposals.iter().map(|p| match p.gov_action().kind() {
            MultiEraGovActionKind::UpdateCommittee(.., threshold) => Quorum::Update(Some((
                threshold.numerator.into(),
                threshold.denominator.into(),
            ))),
            _ => Quorum::Other,
        });
        LedgerTx {
            fee: tx.fee(),
            scripts: scripts.sorted().collect(),
            quorum: quorum.collect(),
        }
    }
}

/// The fee, witness scripts and committee thresholds a ReadTx version maps for a transaction, with no fee in a message that has no fee field.
struct MappedTx {
    fee: Option<WireFee>,
    scripts: Vec<ScriptKey>,
    quorum: Vec<Quorum>,
}

/// The language and hash of a Plutus script a message carries the bytes of.
fn plutus(language: ScriptLanguage, bytes: &[u8]) -> ScriptKey {
    let hash = *Hasher::<224>::hash_tagged(bytes, language.tag());
    Some((language, Some(hash)))
}

/// The fields one u5c version maps for a transaction message, with the given fee.
macro_rules! mapped {
    ($version:ident, $tx:expr, $fee:expr) => {{
        use pallas::interop::utxorpc::$version::spec::cardano::{
            governance_action::GovernanceAction, script::Script,
        };
        let tx = $tx;
        let witnessed = tx.witnesses.iter().flat_map(|w| &w.script);
        let scripts = witnessed.map(|s| match &s.script {
            None => None,
            Some(Script::Native(_)) => Some((ScriptLanguage::Native, None)),
            Some(Script::PlutusV1(b)) => plutus(ScriptLanguage::PlutusV1, b),
            Some(Script::PlutusV2(b)) => plutus(ScriptLanguage::PlutusV2, b),
            Some(Script::PlutusV3(b)) => plutus(ScriptLanguage::PlutusV3, b),
            Some(Script::PlutusV4(b)) => plutus(ScriptLanguage::PlutusV4, b),
        });
        let quorum = tx.proposals.iter().map(|p| {
            let action = p.gov_action.as_ref();
            match action.and_then(|a| a.governance_action.as_ref()) {
                Some(GovernanceAction::UpdateCommitteeAction(a)) => {
                    let threshold = a.new_committee_threshold.as_ref();
                    Quorum::Update(threshold.map(|r| (r.numerator.into(), r.denominator.into())))
                }
                _ => Quorum::Other,
            }
        });
        MappedTx {
            fee: $fee,
            scripts: scripts.sorted().collect(),
            quorum: quorum.collect(),
        }
    }};
}

/// What a ReadTx version answers for a transaction hash.
enum Answer<T> {
    /// The mapped transaction.
    Mapped(T),
    /// An answer that holds no transaction where the ledger names one.
    Absent,
    /// The status the version answers in place of a transaction.
    Status(tonic::Code),
    /// The message of a panic in the version's mapping.
    Panicked(String),
}

impl<T> Answer<T> {
    /// The answer for a part of the mapped transaction, or this answer's absence, status or panic.
    fn then<U>(&self, part: impl FnOnce(&T) -> Answer<U>) -> Answer<U> {
        match self {
            Answer::Mapped(tx) => part(tx),
            Answer::Absent => Answer::Absent,
            Answer::Status(code) => Answer::Status(*code),
            Answer::Panicked(message) => Answer::Panicked(message.clone()),
        }
    }
}

/// The ReadTx answer of one u5c version for a transaction hash against the domain, with a panic of its mapping caught.
macro_rules! read_tx {
    ($version:ident, $domain:expr, $hash:expr) => {{
        use pallas::interop::utxorpc::$version::spec::query::{
            any_chain_tx::Chain, query_service_server::QueryService as _, ReadTxRequest,
        };
        let request = ReadTxRequest {
            hash: $hash.to_vec().into(),
            ..Default::default()
        };
        let service = crate::serve::grpc::$version::query::QueryServiceImpl::new($domain.clone());
        let answer = AssertUnwindSafe(service.read_tx(Request::new(request)))
            .catch_unwind()
            .await;
        match answer {
            Err(payload) => Answer::Panicked(panic_message(payload).replace('\n', " ")),
            Ok(Err(status)) => Answer::Status(status.code()),
            Ok(Ok(answer)) => match answer.into_inner().tx.and_then(|tx| tx.chain) {
                Some(Chain::Cardano(tx)) => Answer::Mapped(tx),
                None => Answer::Absent,
            },
        }
    }};
}

/// A field the judge compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Fee,
    Scripts,
    Quorum,
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Field::Fee => "fee",
            Field::Scripts => "scripts",
            Field::Quorum => "quorum",
        })
    }
}

/// The witness scripts as a line shows them, each by its language and the first four bytes of its hash.
fn show_scripts(scripts: &[ScriptKey]) -> String {
    let mut shown = scripts.iter().map(|s| match s {
        None => "empty".to_string(),
        Some((language, None)) => format!("{language:?}"),
        Some((language, Some(hash))) => format!("{language:?}:{}", hex::encode(&hash[..4])),
    });
    shown.join(",")
}

/// Each field the mapping holds a different value in than the ledger, with the two values of each.
fn differing(ledger: &LedgerTx, mapped: &MappedTx) -> Vec<(Field, String, String)> {
    let mut differing = vec![];
    if let Some(fee) = &mapped.fee {
        let holds = match (ledger.fee, fee) {
            (None, WireFee::Unset) => true,
            (Some(n), WireFee::Int(m)) => i64::try_from(n) == Ok(*m),
            _ => false,
        };
        if !holds {
            differing.push((Field::Fee, format!("{:?}", ledger.fee), format!("{fee:?}")));
        }
    }
    if ledger.scripts != mapped.scripts {
        let (l, m) = (show_scripts(&ledger.scripts), show_scripts(&mapped.scripts));
        differing.push((Field::Scripts, l, m));
    }
    let (l, m) = (&ledger.quorum, &mapped.quorum);
    if l.len() != m.len() || l.iter().zip(m).any(|(l, m)| !l.holds(*m)) {
        differing.push((Field::Quorum, l.iter().join(","), m.iter().join(",")));
    }
    differing
}

/// Adds the comparison of what one version answers for one transaction the ledger accepted.
fn add(
    tally: &mut Tally,
    id: RecordId,
    layer: Layer,
    name: TxName,
    ledger: &LedgerTx,
    answer: Answer<MappedTx>,
) {
    let (agrees, dolos) = match answer {
        Answer::Mapped(mapped) => {
            let differing = differing(ledger, &mapped);
            let fields = differing.iter().map(|(f, _, _)| f).join(",");
            let mut ledger = differing.iter().map(|(f, l, _)| format!("{f}={l}"));
            let ledger = ledger.join(" ");
            let mut dolos = differing.iter().map(|(f, _, d)| format!("{f}={d}"));
            let dolos = dolos.join(" ");
            let shown = format!("differing={fields} ledger=[{ledger}] dolos=[{dolos}]");
            (differing.is_empty(), shown)
        }
        Answer::Absent => (false, "dolos=none".to_string()),
        Answer::Status(code) => (false, format!("status={code:?}")),
        Answer::Panicked(message) => (false, format!("panicked={message}")),
    };
    tally.add(agrees, || format!("{id} {layer} {name} {dolos}"));
}

/// Evidence that ReadTx v1 mapped each accepted transaction of a run without a panic, which the passes that serve the run through v1 take.
pub(super) struct V1Maps(());

/// Counts what each ReadTx version maps for the accepted records of the blocks a run rolled against the ledger's transactions, v1 by each top level hash with its sub transactions and v1beta and v1alpha by each top level and sub transaction hash, with the evidence that v1 mapped them when no v1 mapping panicked.
pub(super) async fn compare(
    domain: &DomainAdapter,
    kind: CorpusKind,
    blocks: &[GroupBlock<'_>],
    tally: &mut Tally,
) -> Option<V1Maps> {
    let mut v1 = Some(V1Maps(()));
    let groups = blocks.iter().filter_map(GroupBlock::rolled);
    let accepted = groups
        .flat_map(|rolled| rolled.group)
        .filter(|r| matches!(r.outcome, LedgerOutcome::Accepted { .. }));
    for record in accepted {
        let id = RecordId {
            corpus: kind,
            n: record.n,
        };
        let tx = MultiEraTx::decode_for_era(Era::Dijkstra, &record.tx.0)
            .unwrap_or_else(|e| panic!("TX {id} {e}"));
        let top = (*tx.hash(), LedgerTx::of(&tx));
        let subs = tx.sub_transactions();
        let subs: Vec<([u8; 32], LedgerTx)> = subs
            .iter()
            .map(|sub| (*sub.hash(), LedgerTx::of(sub)))
            .collect();
        let served = read_tx!(v1, domain, top.0);
        if let Answer::Panicked(_) = served {
            v1 = None;
        }
        let common = served.then(|tx| {
            let fee = wire_fee!(v1, tx.fee.clone());
            match &tx.common {
                Some(common) => Answer::Mapped(mapped!(v1, common, Some(fee))),
                None => Answer::Absent,
            }
        });
        add(tally, id, Layer::V1, TxName::Top(top.0), &top.1, common);
        for (hash, ledger) in &subs {
            let common = served.then(|tx| {
                let sub = tx.sub_transactions.iter().find(|c| c.hash == hash[..]);
                match sub {
                    Some(common) => Answer::Mapped(mapped!(v1, common, None)),
                    None => Answer::Absent,
                }
            });
            add(tally, id, Layer::V1, TxName::Sub(*hash), ledger, common);
        }
        let names = std::iter::once((TxName::Top(top.0), &top.1));
        let names = names.chain(subs.iter().map(|(hash, l)| (TxName::Sub(*hash), l)));
        for (name, ledger) in names {
            let hash = match name {
                TxName::Top(h) | TxName::Sub(h) => h,
            };
            let answer = read_tx!(v1beta, domain, hash).then(|tx| {
                let fee = wire_fee!(v1beta, tx.fee.clone());
                Answer::Mapped(mapped!(v1beta, tx, Some(fee)))
            });
            add(tally, id, Layer::V1beta, name, ledger, answer);
            let answer = read_tx!(v1alpha, domain, hash).then(|tx| {
                let fee = wire_fee!(v1alpha, tx.fee.clone());
                Answer::Mapped(mapped!(v1alpha, tx, Some(fee)))
            });
            add(tally, id, Layer::V1alpha, name, ledger, answer);
        }
    }
    v1
}

#[test]
fn a_threshold_the_fields_round_holds_and_a_truncated_one_does_not() {
    let third = Quorum::Update(Some((1, 3)));
    assert!(third.holds(Quorum::Update(Some((715_827_882, 2_147_483_647)))));
    assert!(Quorum::Update(Some((5, 0))).holds(Quorum::Update(Some((1, 0)))));
    assert!(Quorum::Other.holds(Quorum::Other));
    let (n, d): (u64, u64) = (1_435_934_570_883, 2_500_000_000_000);
    let truncated = Quorum::Update(Some(((n as i32).into(), (d as u32).into())));
    assert!(!Quorum::Update(Some((n.into(), d.into()))).holds(truncated));
    assert!(!third.holds(Quorum::Update(None)));
    assert!(!third.holds(Quorum::Other));
}

fn ledger(fee: Option<u64>) -> LedgerTx {
    LedgerTx {
        fee,
        scripts: vec![
            Some((ScriptLanguage::Native, None)),
            Some((ScriptLanguage::PlutusV3, Some([3; 28]))),
        ],
        quorum: vec![Quorum::Other],
    }
}

fn mapped(fee: Option<WireFee>) -> MappedTx {
    let ledger = ledger(None);
    MappedTx {
        fee,
        scripts: ledger.scripts,
        quorum: ledger.quorum,
    }
}

fn fields(ledger: &LedgerTx, mapped: &MappedTx) -> Vec<Field> {
    let differing = differing(ledger, mapped);
    differing.into_iter().map(|d| d.0).collect()
}

#[test]
fn a_fee_the_ledger_holds_none_of_differs_when_mapped_as_zero() {
    let none: Vec<Field> = vec![];
    let (unset, zero) = (mapped(Some(WireFee::Unset)), mapped(Some(WireFee::Int(0))));
    assert_eq!(fields(&ledger(None), &unset), none);
    let fee = Some(WireFee::Int(170_000));
    assert_eq!(fields(&ledger(Some(170_000)), &mapped(fee)), none);
    assert_eq!(fields(&ledger(Some(7)), &mapped(None)), none);
    assert_eq!(fields(&ledger(None), &zero), [Field::Fee]);
    assert_eq!(fields(&ledger(Some(7)), &unset), [Field::Fee]);
}

#[test]
fn a_dropped_witness_script_or_proposal_differs() {
    let ledger = ledger(None);
    let mut dropped = mapped(None);
    dropped.scripts.pop();
    assert_eq!(fields(&ledger, &dropped), [Field::Scripts]);
    let mut dropped = mapped(None);
    dropped.quorum.clear();
    assert_eq!(fields(&ledger, &dropped), [Field::Quorum]);
}
