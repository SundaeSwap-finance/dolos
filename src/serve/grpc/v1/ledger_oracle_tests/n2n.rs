//! Judges a Leios peer's node to node session against the blocks Dolos serves.

use std::{
    collections::VecDeque,
    fmt, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use pallas::{
    codec::minicbor,
    crypto::hash::Hash,
    ledger::traverse::MultiEraHeader,
    network2::{
        behavior::initiator::{DisconnectReason, InitiatorCommand, InitiatorEvent},
        protocol::{
            handshake::{
                n2n::{VersionData, VersionTable, LEIOS_MIN_VERSION},
                Message, RefuseReason,
            },
            leiosfetch,
            leiosnotify::Notification,
        },
    },
};
use serde::de::DeserializeOwned;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpStream,
    time::{timeout, timeout_at, Instant},
};

use super::super::spec::sync as s;
use super::endorser::{block_txs, listed, tx_id, Listed};
use crate::sync::{
    leios::{
        proposal, AnnouncedEndorserBlock, Error, LeiosClient, LeiosTransport, ManagerTransport,
        FETCH_TIMEOUT, TXS_PER_REQUEST,
    },
    pull::announced_by,
};

/// How long the session waits for the handshake, and then for the first
/// notification.
const SESSION_WAIT: Duration = Duration::from_secs(60);

/// What a peer session delivered, recorded as each event arrived.
#[derive(Debug, Default)]
struct Heard {
    accepted: Option<(u64, VersionData)>,
    ended: Option<DisconnectReason>,
    announcements: Vec<Vec<u8>>,
    offers: usize,
    txs_offers: usize,
    votes: usize,
    bodies: Vec<Vec<u8>>,
}

impl Heard {
    fn notifications(&self) -> usize {
        self.announcements.len() + self.offers + self.txs_offers + self.votes
    }

    /// The announcements that decode as a Dijkstra header announcing an
    /// endorser block.
    fn announcing(&self) -> usize {
        self.announcements
            .iter()
            .filter(|cbor| {
                MultiEraHeader::decode(7, None, cbor)
                    .is_ok_and(|header| header.eb_announcement().is_some())
            })
            .count()
    }
}

/// A transport that records every event the inner transport delivers and
/// holds the ones read before a fetch, so the fetch still reads them.
struct Recording<T> {
    inner: T,
    heard: Heard,
    held: VecDeque<InitiatorEvent>,
}

impl<T: LeiosTransport> Recording<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            heard: Heard::default(),
            held: VecDeque::new(),
        }
    }

    async fn pull(&mut self) -> InitiatorEvent {
        let event = self.inner.next_event().await;
        match &event {
            InitiatorEvent::PeerInitialized(_, accepted) => {
                self.heard.accepted = Some(accepted.clone())
            }
            InitiatorEvent::PeerDisconnected(_, reason) => self.heard.ended = Some(*reason),
            InitiatorEvent::EbNotification(_, notification) => match notification {
                Notification::BlockAnnouncement(header) => {
                    self.heard.announcements.push(header.raw_bytes().to_vec())
                }
                Notification::BlockOffer(..) => self.heard.offers += 1,
                Notification::BlockTxsOffer(..) => self.heard.txs_offers += 1,
                Notification::Votes(votes) => self.heard.votes += votes.len(),
            },
            InitiatorEvent::EbFetched(_, _, leiosfetch::Response::Block(body)) => {
                self.heard.bodies.push(body.raw_bytes().to_vec())
            }
            _ => (),
        }
        event
    }

    /// Holds events until `enough` holds of what was heard, the session ends
    /// or `limit` passes.
    async fn wait(&mut self, limit: Duration, enough: fn(&Heard) -> bool) {
        let until = Instant::now() + limit;
        while !enough(&self.heard) && self.heard.ended.is_none() {
            match timeout_at(until, self.pull()).await {
                Ok(event) => self.held.push_back(event),
                Err(_) => return,
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<T: LeiosTransport> LeiosTransport for Recording<T> {
    fn issue(&mut self, command: InitiatorCommand) {
        self.inner.issue(command)
    }

    async fn next_event(&mut self) -> InitiatorEvent {
        match self.held.pop_front() {
            Some(event) => event,
            None => self.pull().await,
        }
    }
}

/// How the handshake of a session ended, as the client's events show it.
#[derive(Debug, PartialEq)]
enum Handshake {
    /// The peer accepted a version that carries Leios, with the magic asked
    /// for.
    Leios(u64, VersionData),
    /// The peer accepted a version below the one that carries Leios.
    PreLeios(u64, VersionData),
    /// The peer accepted with another network magic.
    OtherMagic(u64, VersionData),
    /// The session ended before the peer accepted a version.
    Refused(DisconnectReason),
    /// Neither an accepted version nor an end arrived in time.
    Silent,
}

impl Handshake {
    fn of(heard: &Heard, magic: u64) -> Self {
        match (&heard.accepted, heard.ended) {
            (Some((version, data)), _) if data.network_magic != magic => {
                Self::OtherMagic(*version, data.clone())
            }
            (Some((version, data)), _) if *version < LEIOS_MIN_VERSION => {
                Self::PreLeios(*version, data.clone())
            }
            (Some((version, data)), _) => Self::Leios(*version, data.clone()),
            (None, Some(reason)) => Self::Refused(reason),
            (None, None) => Self::Silent,
        }
    }

    fn passed(&self) -> bool {
        matches!(self, Self::Leios(..))
    }

    fn reason(&self) -> &'static str {
        match self {
            Self::Leios(..) => "leios",
            Self::PreLeios(..) => "pre-leios",
            Self::OtherMagic(..) => "other-magic",
            Self::Refused(..) => "refused",
            Self::Silent => "silent",
        }
    }
}

fn shown<T: fmt::Display>(value: Option<T>) -> String {
    value.map_or("none".to_string(), |v| v.to_string())
}

fn version_data(f: &mut fmt::Formatter<'_>, version: u64, data: &VersionData) -> fmt::Result {
    write!(
        f,
        " version={version} magic={} initiator_only_diffusion_mode={} peer_sharing={} query={} \
         peras_support={}",
        data.network_magic,
        data.initiator_only_diffusion_mode,
        shown(data.peer_sharing),
        shown(data.query),
        shown(data.peras_support),
    )
}

impl fmt::Display for Handshake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verdict = if self.passed() { "PASS" } else { "FAIL" };
        write!(f, "handshake={verdict} reason={}", self.reason())?;
        match self {
            Self::Leios(version, data)
            | Self::PreLeios(version, data)
            | Self::OtherMagic(version, data) => version_data(f, *version, data),
            Self::Refused(reason) => write!(f, " ended={reason:?}"),
            Self::Silent => Ok(()),
        }
    }
}

/// The handshake proposal message offering `table`, as it goes on the wire.
fn proposed(table: VersionTable) -> Vec<u8> {
    minicbor::to_vec(Message::Propose(table)).expect("a proposal encodes")
}

/// The proposals whose replies the fixture records: the head's, and the head's
/// with each defect a client could make in it.
fn recorded_proposals(magic: u64) -> Vec<(&'static str, Vec<u8>)> {
    let mut four_fields = proposal(magic);
    for data in four_fields.values.values_mut() {
        data.peras_support = None;
    }
    vec![
        ("head", proposed(proposal(magic))),
        ("v16-four-fields", proposed(four_fields)),
        ("other-magic", proposed(proposal(magic + 1))),
    ]
}

/// The peer's reply to one handshake proposal sent alone on a fresh
/// connection, read from its handshake segments until it decodes.
async fn reply_to(peer: &str, proposal: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(peer).await?;
    let mut segment = vec![0u8; 4];
    segment.extend_from_slice(&0u16.to_be_bytes());
    segment.extend_from_slice(&(proposal.len() as u16).to_be_bytes());
    segment.extend_from_slice(proposal);
    stream.write_all(&segment).await?;
    let mut reply = Vec::new();
    loop {
        let mut header = [0u8; 8];
        stream.read_exact(&mut header).await?;
        let mut payload = vec![0u8; u16::from_be_bytes([header[6], header[7]]) as usize];
        stream.read_exact(&mut payload).await?;
        if u16::from_be_bytes([header[4], header[5]]) & 0x7fff == 0 {
            reply.extend(payload);
        }
        if minicbor::decode::<Message<VersionData>>(&reply).is_ok() {
            return Ok(reply);
        }
    }
}

/// What a handshake reply says of the proposal it answers.
#[derive(Debug, PartialEq)]
enum Reply {
    Accepted(u64, VersionData),
    VersionMismatch(Vec<u64>),
    DecodeError(u64, String),
    Refused(u64, String),
    Query,
    Undecoded(String),
}

impl Reply {
    fn of(bytes: &[u8]) -> Self {
        match minicbor::decode::<Message<VersionData>>(bytes) {
            Ok(Message::Accept(version, data)) => Self::Accepted(version, data),
            Ok(Message::Refuse(RefuseReason::VersionMismatch(versions))) => {
                Self::VersionMismatch(versions)
            }
            Ok(Message::Refuse(RefuseReason::HandshakeDecodeError(version, text))) => {
                Self::DecodeError(version, text)
            }
            Ok(Message::Refuse(RefuseReason::Refused(version, text))) => {
                Self::Refused(version, text)
            }
            Ok(Message::QueryReply(_) | Message::Propose(_)) => Self::Query,
            Err(e) => Self::Undecoded(e.to_string()),
        }
    }
}

/// A recorded reply of the peer to one proposal.
struct RecordedReply {
    name: String,
    proposal: Vec<u8>,
    reply: Vec<u8>,
}

fn recorded_replies(text: &str) -> Vec<RecordedReply> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut words = line.split_whitespace();
            let mut next = || words.next().expect("a name, a proposal and a reply");
            RecordedReply {
                name: next().to_string(),
                proposal: hex::decode(next()).expect("a proposal in hex"),
                reply: hex::decode(next()).expect("a reply in hex"),
            }
        })
        .collect()
}

/// A certified endorser block and the transactions Dolos serves in the block
/// that certifies it.
#[derive(Debug, Clone, PartialEq)]
struct Certified {
    certifying_slot: u64,
    announced: AnnouncedEndorserBlock,
    served: Vec<Vec<u8>>,
}

/// One endorser block as the peer delivered it, beside the transactions Dolos
/// serves for it.
#[derive(Debug, PartialEq)]
struct RecordedFetch {
    certified: Certified,
    body: Vec<u8>,
    txs: Vec<Vec<u8>>,
}

impl RecordedFetch {
    fn render(&self) -> String {
        let announced = &self.certified.announced;
        let mut lines = vec![
            format!(
                "certified {} {} {} {}",
                self.certified.certifying_slot, announced.slot, announced.hash, announced.size
            ),
            format!("body {}", hex::encode(&self.body)),
        ];
        lines.extend(self.txs.iter().map(|tx| format!("tx {}", hex::encode(tx))));
        lines.extend(
            self.certified
                .served
                .iter()
                .map(|tx| format!("served {}", hex::encode(tx))),
        );
        lines.join("\n") + "\n"
    }

    fn parse(text: &str) -> Self {
        let mut certified = None;
        let mut body = Vec::new();
        let mut txs = Vec::new();
        let mut served = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let (kind, rest) = line.split_once(' ').expect("a kind and its value");
            match kind {
                "certified" => {
                    let words: Vec<&str> = rest.split_whitespace().collect();
                    let number = |i: usize| words[i].parse::<u64>().expect("a number");
                    certified = Some((
                        number(0),
                        AnnouncedEndorserBlock {
                            slot: number(1),
                            hash: words[2].parse::<Hash<32>>().expect("a hash"),
                            size: number(3) as u32,
                        },
                    ));
                }
                "body" => body = hex::decode(rest).expect("a body in hex"),
                "tx" => txs.push(hex::decode(rest).expect("a transaction in hex")),
                "served" => served.push(hex::decode(rest).expect("a transaction in hex")),
                other => panic!("an unknown line kind {other}"),
            }
        }
        let (certifying_slot, announced) = certified.expect("a certified line");
        Self {
            certified: Certified {
                certifying_slot,
                announced,
                served,
            },
            body,
            txs,
        }
    }
}

/// How one fetched endorser block compares with the transactions Dolos serves
/// for it.
#[derive(Debug, PartialEq)]
enum Fetched {
    Same(usize),
    Differs {
        index: usize,
        fetched: Option<Vec<u8>>,
        served: Option<Vec<u8>>,
    },
    Failed {
        reason: &'static str,
        detail: String,
    },
}

impl Fetched {
    fn of(fetched: Result<Vec<Vec<u8>>, Error>, served: &[Vec<u8>]) -> Self {
        match fetched {
            Ok(fetched) => match listed(&fetched, served) {
                Listed::Same(count) => Self::Same(count),
                Listed::Differs {
                    index,
                    served: fetched,
                    node: served,
                } => Self::Differs {
                    index,
                    fetched,
                    served,
                },
            },
            Err(error) => Self::Failed {
                reason: match error {
                    Error::PreLeiosPeer { .. } => "pre-leios",
                    Error::Disconnected { .. } => "disconnected",
                    Error::Timeout { .. } => "timeout",
                    _ => "refused",
                },
                detail: error.to_string(),
            },
        }
    }

    fn passed(&self) -> bool {
        matches!(self, Self::Same(_))
    }
}

fn id(tx: &Option<Vec<u8>>) -> String {
    match tx {
        None => "none".to_string(),
        Some(tx) => tx_id(tx).map_or_else(|e| format!("undecoded({e})"), |id| id.to_string()),
    }
}

impl fmt::Display for Fetched {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Same(count) => write!(f, "fetch=PASS reason=same txs={count}"),
            Self::Differs {
                index,
                fetched,
                served,
            } => write!(
                f,
                "fetch=FAIL reason=differs index={index} fetched={} served={}",
                id(fetched),
                id(served)
            ),
            Self::Failed { reason, detail } => {
                write!(f, "fetch=FAIL reason={reason} detail=\"{detail}\"")
            }
        }
    }
}

/// What one session with the peer showed.
#[derive(Debug)]
struct Session {
    handshake: Handshake,
    heard: Heard,
    fetched: Vec<(Certified, Fetched)>,
}

impl Session {
    fn passed(&self) -> bool {
        self.handshake.passed()
            && self.heard.notifications() > 0
            && !self.fetched.is_empty()
            && self.fetched.iter().all(|(_, f)| f.passed())
    }
}

/// Runs one session over `transport`: the handshake, the first notification,
/// then a fetch of each certified endorser block until one fails.
async fn session<T: LeiosTransport>(
    transport: T,
    peer: &str,
    magic: u64,
    certified: &[Certified],
    wait: Duration,
) -> Session {
    let mut recording = Recording::new(transport);
    recording.wait(wait, |h| h.accepted.is_some()).await;
    let handshake = Handshake::of(&recording.heard, magic);
    recording.wait(wait, |h| h.notifications() > 0).await;

    let mut fetched = Vec::new();
    if handshake.passed() {
        let mut client = LeiosClient::with_transport(
            &mut recording,
            peer.to_string(),
            TXS_PER_REQUEST,
            FETCH_TIMEOUT,
        );
        for block in certified {
            let compared = Fetched::of(client.fetch(&block.announced).await, &block.served);
            let failed = !compared.passed();
            fetched.push((block.clone(), compared));
            if failed {
                break;
            }
        }
    }

    Session {
        handshake,
        heard: recording.heard,
        fetched,
    }
}

/// The endorser blocks the served blocks certify, for each certifying block
/// whose parent was also read, with a line for each certifying block whose
/// parent was not.
fn certified(natives: &[Vec<u8>]) -> (Vec<Certified>, Vec<String>) {
    let headers: Vec<MultiEraHeader> = natives
        .iter()
        .map(|native| {
            MultiEraHeader::decode_from_block(native)
                .unwrap_or_else(|e| panic!("N2N a served block's header does not decode: {e}"))
        })
        .collect();
    let mut found = Vec::new();
    let mut unpaired = Vec::new();
    for (native, header) in natives.iter().zip(&headers) {
        if header.block_body_contains_leios_cert() != Some(true) {
            continue;
        }
        let Some(parent) = headers
            .iter()
            .find(|p| Some(p.hash()) == header.previous_hash())
        else {
            unpaired.push(format!(
                "N2N certifying_slot={} parent=unread",
                header.slot()
            ));
            continue;
        };
        let announced = announced_by(Some(parent), header)
            .unwrap_or_else(|e| panic!("N2N certifying_slot={} {e}", header.slot()))
            .unwrap_or_else(|| panic!("N2N certifying_slot={} certifies nothing", header.slot()));
        let served = block_txs(native)
            .unwrap_or_else(|e| panic!("N2N certifying_slot={} {e}", header.slot()));
        found.push(Certified {
            certifying_slot: header.slot(),
            announced,
            served,
        });
    }
    (found, unpaired)
}

fn read<T: DeserializeOwned>(dir: &Path, name: &str) -> T {
    let path = dir.join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("N2N {} {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("N2N {} {e}", path.display()))
}

/// The files in the reads directory whose names start with the prefix, in name
/// order.
fn files(dir: &Path, prefix: &str) -> Vec<String> {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("N2N {} {e}", dir.display()));
    let mut names: Vec<String> = entries
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(prefix))
        .collect();
    names.sort();
    names
}

/// The native bytes of each block the reads dir holds a FetchBlock answer for.
fn served_natives(dir: &Path) -> Vec<Vec<u8>> {
    let mut names = files(dir, "block-");
    names.extend(files(dir, "parent-"));
    names
        .iter()
        .map(|name| {
            let response: s::FetchBlockResponse = read(dir, name);
            response
                .block
                .unwrap_or_else(|| panic!("N2N {name} serves no block"))
                .native_bytes
                .to_vec()
        })
        .collect()
}

/// The reads dir the environment variable `var` names, and the address and
/// magic of the peer its `peer` file names.
fn reads(var: &str) -> (PathBuf, String, u64) {
    let dir = PathBuf::from(
        std::env::var(var).unwrap_or_else(|_| panic!("{var} names the reads directory")),
    );
    let text = std::fs::read_to_string(dir.join("peer")).expect("N2N peer");
    let (peer, magic) = text.trim().split_once(' ').expect("an address and a magic");
    let magic = magic.parse().expect("a network magic");
    (dir, peer.to_string(), magic)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks node to node with the peer LIVE_DIR/peer names and judges it against the FetchBlock answers oracle/box/n2n.sh wrote there"]
async fn n2n() {
    let (dir, peer, magic) = reads("LIVE_DIR");
    let natives = served_natives(&dir);
    let (certified, unpaired) = certified(&natives);
    for line in &unpaired {
        println!("{line}");
    }

    let transport = ManagerTransport::connect(&peer, magic, |_| true).expect("N2N peer address");
    let session = session(transport, &peer, magic, &certified, SESSION_WAIT).await;

    println!(
        "N2N {} peer={peer} asked_magic={magic} node_version=none",
        session.handshake
    );
    println!(
        "N2N notify={} announcements={} announcing={} offers={} txs_offers={} votes={}",
        if session.heard.notifications() > 0 {
            "PASS"
        } else {
            "FAIL"
        },
        session.heard.announcements.len(),
        session.heard.announcing(),
        session.heard.offers,
        session.heard.txs_offers,
        session.heard.votes,
    );
    for (block, fetched) in &session.fetched {
        println!(
            "N2N {fetched} certifying_slot={} eb_slot={} eb={} size={} served_txs={}",
            block.certifying_slot,
            block.announced.slot,
            block.announced.hash,
            block.announced.size,
            block.served.len()
        );
    }
    println!(
        "N2N RESULT {} served_blocks={} certified={} fetched={} ended={}",
        if session.passed() { "PASS" } else { "FAIL" },
        natives.len(),
        certified.len(),
        session.fetched.len(),
        session
            .heard
            .ended
            .map_or("no".to_string(), |r| format!("{r:?}"))
    );
    assert!(session.passed(), "the node to node session disagrees");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "records the replies of the peer FIXTURE_DIR/peer names to each recorded proposal, and its endorser block certified at the slot FIXTURE_DIR/slot names, into FIXTURE_DIR/out"]
async fn record_n2n() {
    let (dir, peer, magic) = reads("FIXTURE_DIR");
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();

    let mut lines = Vec::new();
    for (name, proposal) in recorded_proposals(magic) {
        let reply = timeout(SESSION_WAIT, reply_to(&peer, &proposal))
            .await
            .unwrap_or_else(|_| panic!("N2N no reply to {name}"))
            .unwrap_or_else(|e| panic!("N2N {name} {e}"));
        println!("N2N recorded {name} {:?}", Reply::of(&reply));
        lines.push(format!(
            "{name} {} {}",
            hex::encode(&proposal),
            hex::encode(&reply)
        ));
    }
    std::fs::write(out.join("handshake.txt"), lines.join("\n") + "\n").unwrap();

    let slot: u64 = std::fs::read_to_string(dir.join("slot"))
        .expect("N2N slot")
        .trim()
        .parse()
        .expect("a slot");
    let (certified, _) = certified(&served_natives(&dir));
    let certified = certified
        .into_iter()
        .find(|c| c.certifying_slot == slot)
        .unwrap_or_else(|| panic!("N2N no block read certifies at slot {slot}"));
    let transport = ManagerTransport::connect(&peer, magic, |_| true).expect("N2N peer address");
    let mut recording = Recording::new(transport);
    let txs =
        LeiosClient::with_transport(&mut recording, peer.clone(), TXS_PER_REQUEST, FETCH_TIMEOUT)
            .fetch(&certified.announced)
            .await
            .unwrap_or_else(|e| panic!("N2N {e}"));
    let body = recording.heard.bodies.pop().expect("N2N a fetched body");
    let fetch = RecordedFetch {
        certified,
        body,
        txs,
    };
    println!(
        "N2N recorded fetch certifying_slot={} txs={} served={}",
        fetch.certified.certifying_slot,
        fetch.txs.len(),
        fetch.certified.served.len()
    );
    std::fs::write(out.join("fetch.txt"), fetch.render()).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::leios::tests::{endorser_block_responder, real_eb};

    /// The magic of the network the fixtures were recorded on.
    const MAGIC: u64 = 164;

    const LOCAL_WAIT: Duration = Duration::from_secs(10);

    fn recorded() -> Vec<RecordedReply> {
        recorded_replies(include_str!("../../../../../test_data/n2n/handshake.txt"))
    }

    fn recorded_fetch() -> RecordedFetch {
        RecordedFetch::parse(include_str!("../../../../../test_data/n2n/fetch.txt"))
    }

    fn reply_named(name: &str) -> Reply {
        let recorded = recorded();
        let found = recorded.iter().find(|r| r.name == name).unwrap();
        Reply::of(&found.reply)
    }

    /// The node accepts the proposal the head makes at a version that carries
    /// Leios, with the magic asked for and the version data that version
    /// requires.
    #[test]
    fn the_node_accepts_the_proposal_at_the_leios_version() {
        let sent = proposed(proposal(MAGIC));
        let recorded = recorded();
        let Some(answered) = recorded.iter().find(|r| r.proposal == sent) else {
            panic!("the node's reply to {} is not recorded", hex::encode(&sent));
        };
        match Reply::of(&answered.reply) {
            Reply::Accepted(version, data) => {
                assert!(version >= LEIOS_MIN_VERSION, "accepted version {version}");
                assert_eq!(data.network_magic, MAGIC);
                assert!(data.peras_support.is_some(), "{data:?}");
            }
            Reply::VersionMismatch(versions) => {
                panic!("the node offers none of the versions, only {versions:?}")
            }
            Reply::DecodeError(version, text) => {
                panic!("the node cannot decode the version data of {version}: {text}")
            }
            Reply::Refused(version, text) => panic!("the node refuses version {version}: {text}"),
            other => panic!("the node answers {other:?}"),
        }
    }

    /// The recorded replies say the node refuses each defective proposal.
    #[test]
    fn the_node_refuses_each_defective_proposal() {
        assert!(matches!(reply_named("head"), Reply::Accepted(16, _)));
        assert!(
            !matches!(reply_named("v16-four-fields"), Reply::Accepted(16, _)),
            "{:?}",
            reply_named("v16-four-fields")
        );
        assert!(
            matches!(reply_named("other-magic"), Reply::Refused(..)),
            "{:?}",
            reply_named("other-magic")
        );
    }

    /// The recorded proposals are the ones the head makes today.
    #[test]
    fn the_recorded_proposals_are_the_heads() {
        let recorded = recorded();
        let made = recorded_proposals(MAGIC);
        assert_eq!(recorded.len(), made.len());
        for ((name, proposal), recorded) in made.iter().zip(&recorded) {
            assert_eq!(*name, recorded.name);
            assert_eq!(*proposal, recorded.proposal, "{name}");
        }
    }

    /// An endorser block recorded from the node is fetched whole and equal to
    /// the transactions Dolos serves in the block that certifies it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_recorded_endorser_block_is_fetched_as_dolos_serves_it() {
        let recorded = recorded_fetch();
        let (address, _) = endorser_block_responder(recorded.body, recorded.txs).await;
        let mut client = LeiosClient::new(&address.to_string(), MAGIC).unwrap();

        let fetched = client
            .fetch(&recorded.certified.announced)
            .await
            .expect("the recorded endorser block is fetched");

        assert_eq!(
            listed(&fetched, &recorded.certified.served),
            Listed::Same(recorded.certified.served.len())
        );
    }

    /// A recorded fetch reads back as it was written.
    #[test]
    fn a_recorded_fetch_reads_back_as_written() {
        let recorded = recorded_fetch();
        assert_eq!(RecordedFetch::parse(&recorded.render()), recorded);
        assert!(!recorded.certified.served.is_empty());
    }

    fn fixture() -> (Vec<u8>, Vec<Vec<u8>>, Certified) {
        let (body, eb_txs, announced) = real_eb();
        let certified = Certified {
            certifying_slot: announced.slot + 1,
            announced,
            served: eb_txs.clone(),
        };
        (body, eb_txs, certified)
    }

    /// A session with a Leios peer on the magic asked for passes its
    /// handshake at the Leios version, hears the peer's offers and fetches the
    /// endorser block through the recording.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_with_a_leios_peer_passes() {
        let (body, eb_txs, certified) = fixture();
        let (address, _) = endorser_block_responder(body, eb_txs).await;
        let peer = address.to_string();
        let transport = ManagerTransport::connect(&peer, MAGIC, |_| true).unwrap();

        let session = session(transport, &peer, MAGIC, &[certified], LOCAL_WAIT).await;

        assert!(
            matches!(&session.handshake, Handshake::Leios(16, data) if data.peras_support == Some(false)),
            "{}",
            session.handshake
        );
        assert!(session.heard.offers > 0, "{:?}", session.heard);
        assert_eq!(session.fetched.len(), 1);
        assert_eq!(session.fetched[0].1, Fetched::Same(425));
        assert!(session.passed());
    }

    /// A session the peer never accepts fails its handshake and fetches
    /// nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_the_peer_never_accepts_fails_its_handshake() {
        let (body, eb_txs, certified) = fixture();
        let (address, _) = endorser_block_responder(body, eb_txs).await;
        let peer = address.to_string();
        let transport = ManagerTransport::connect(&peer, MAGIC + 1, |_| true).unwrap();

        let session = session(
            transport,
            &peer,
            MAGIC + 1,
            &[certified],
            Duration::from_secs(1),
        )
        .await;

        assert!(
            matches!(session.handshake, Handshake::Refused(_) | Handshake::Silent),
            "{}",
            session.handshake
        );
        assert!(session.fetched.is_empty());
        assert!(!session.passed());
    }

    /// A version below the one that carries Leios, or another magic, fails
    /// the handshake.
    #[test]
    fn a_pre_leios_version_or_another_magic_fails_the_handshake() {
        let data = VersionData::new(MAGIC, true, Some(0), Some(false));
        let heard = |version| Heard {
            accepted: Some((version, data.clone())),
            ..Heard::default()
        };

        assert!(matches!(
            Handshake::of(&heard(15), MAGIC),
            Handshake::PreLeios(15, _)
        ));
        assert!(Handshake::of(&heard(16), MAGIC).passed());
        assert!(matches!(
            Handshake::of(&heard(16), MAGIC + 1),
            Handshake::OtherMagic(16, _)
        ));
    }

    /// A fetch equal to what Dolos serves passes, and one missing a
    /// transaction fails at the index of the first transaction it lacks.
    #[test]
    fn a_fetch_missing_a_transaction_differs_at_its_index() {
        let (_, eb_txs, _) = fixture();
        let mut dropped = eb_txs.clone();
        dropped.remove(212);

        assert_eq!(Fetched::of(Ok(eb_txs.clone()), &eb_txs), Fetched::Same(425));
        assert!(matches!(
            Fetched::of(Ok(dropped), &eb_txs),
            Fetched::Differs { index: 212, .. }
        ));
    }

    /// A fetch the client refuses for the peer's version fails with that
    /// reason.
    #[test]
    fn a_fetch_from_a_pre_leios_peer_fails_with_that_reason() {
        let refused = Fetched::of(
            Err(Error::PreLeiosPeer {
                version: 15,
                minimum: 16,
            }),
            &[],
        );

        assert!(matches!(
            refused,
            Fetched::Failed {
                reason: "pre-leios",
                ..
            }
        ));
    }
}
