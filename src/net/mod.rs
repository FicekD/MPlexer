mod discovery;
mod link;
pub mod proto;

use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::config::Config;
use crate::ddc::{InputSource, StableId};
use crate::model::Owner;
use crate::worker::MonitorSnapshot;
use link::LocalView;
use proto::{Message, MonitorInfo};

const TICK: Duration = Duration::from_secs(1);
const DIAL_COOLDOWN: Duration = Duration::from_secs(4);
const SWITCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Commands from the UI into the net task.
pub enum NetCmd {
    /// Latest local monitor snapshot from the worker.
    LocalState(Vec<MonitorSnapshot>),
    /// Result of a local `SetInput`, used to answer a peer's SwitchRequest.
    LocalSetResult {
        id: StableId,
        result: Result<InputSource, String>,
    },
    /// User asked to switch a monitor the peer owns.
    RequestSwitch { id: StableId, target: InputSource },
}

/// Events from the net task out to the UI.
pub enum NetEvt {
    PeerOnline {
        name: String,
    },
    PeerOffline,
    /// Authoritative owner per local monitor id.
    Owners(HashMap<StableId, Owner>),
    /// The peer asked us (the owner) to switch a monitor; run it on the worker.
    ExecuteSwitch {
        id: StableId,
        target: InputSource,
    },
    /// Reply to a `RequestSwitch` we sent to the peer.
    SwitchResult {
        id: StableId,
        ok: bool,
        error: Option<String>,
    },
    Status(String),
}

/// Internal fan-in for the net task: UI commands, discovery, and link I/O all
/// funnel through one channel so a single thread owns all coordination state.
pub enum NetInbound {
    Cmd(NetCmd),
    Candidate(discovery::Candidate),
    NewConnection {
        stream: TcpStream,
        initiated_by_me: bool,
        peer_id: Uuid,
        peer_name: String,
        peer_monitors: Vec<MonitorInfo>,
    },
    LinkMessage {
        epoch: u64,
        msg: Message,
    },
    LinkClosed {
        epoch: u64,
    },
}

/// UI-facing handle to send commands to the net task.
#[derive(Clone)]
pub struct NetHandle {
    tx: Sender<NetInbound>,
}

impl NetHandle {
    pub fn send(&self, cmd: NetCmd) {
        let _ = self.tx.send(NetInbound::Cmd(cmd));
    }
}

pub fn spawn(config: Config, evt_tx: Sender<NetEvt>, ctx: egui::Context) -> NetHandle {
    let (in_tx, in_rx) = mpsc::channel();
    let handle = NetHandle { tx: in_tx.clone() };
    std::thread::spawn(move || run(config, in_rx, in_tx, evt_tx, ctx));
    handle
}

struct PeerConn {
    epoch: u64,
    peer_id: Uuid,
    initiated_by_me: bool,
    out_tx: Sender<Message>,
}

struct Net {
    config: Config,
    inbound: Sender<NetInbound>,
    evt_tx: Sender<NetEvt>,
    ctx: egui::Context,
    local: LocalView,
    local_snaps: Vec<MonitorSnapshot>,
    peer: Option<PeerConn>,
    peer_monitors: Vec<MonitorInfo>,
    epoch_counter: u64,
    /// Switches we requested from the peer, awaiting a SwitchResult (with send time).
    pending_out: HashMap<StableId, Instant>,
    /// Monitors the peer asked us to switch, awaiting the local SetResult.
    pending_in: HashMap<StableId, ()>,
    last_dial: Option<Instant>,
}

fn run(
    config: Config,
    in_rx: Receiver<NetInbound>,
    in_tx: Sender<NetInbound>,
    evt_tx: Sender<NetEvt>,
    ctx: egui::Context,
) {
    let local: LocalView = Arc::new(Mutex::new(Vec::new()));
    discovery::spawn(
        config.machine_id,
        config.machine_name.clone(),
        link::TCP_PORT,
        in_tx.clone(),
    );
    link::spawn_accept(
        config.machine_id,
        config.machine_name.clone(),
        local.clone(),
        in_tx.clone(),
    );

    let mut net = Net {
        config,
        inbound: in_tx,
        evt_tx,
        ctx,
        local,
        local_snaps: Vec::new(),
        peer: None,
        peer_monitors: Vec::new(),
        epoch_counter: 0,
        pending_out: HashMap::new(),
        pending_in: HashMap::new(),
        last_dial: None,
    };

    loop {
        match in_rx.recv_timeout(TICK) {
            Ok(NetInbound::Cmd(cmd)) => net.handle_cmd(cmd),
            Ok(NetInbound::Candidate(c)) => net.handle_candidate(c),
            Ok(NetInbound::NewConnection {
                stream,
                initiated_by_me,
                peer_id,
                peer_name,
                peer_monitors,
            }) => net.handle_new_connection(stream, initiated_by_me, peer_id, peer_name, peer_monitors),
            Ok(NetInbound::LinkMessage { epoch, msg }) => net.handle_link_message(epoch, msg),
            Ok(NetInbound::LinkClosed { epoch }) => net.handle_link_closed(epoch),
            Err(RecvTimeoutError::Timeout) => net.tick(),
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

impl Net {
    fn emit(&self, evt: NetEvt) {
        if self.evt_tx.send(evt).is_ok() {
            self.ctx.request_repaint();
        }
    }

    fn connected(&self) -> bool {
        self.peer.is_some()
    }

    fn send_to_peer(&self, msg: Message) {
        if let Some(peer) = &self.peer {
            let _ = peer.out_tx.send(msg);
        }
    }

    fn handle_cmd(&mut self, cmd: NetCmd) {
        match cmd {
            NetCmd::LocalState(snaps) => {
                self.local_snaps = snaps;
                self.learn_my_inputs();
                let infos = self.build_my_infos();
                *self.local.lock().unwrap() = infos.clone();
                self.push_owners();
                if self.connected() {
                    self.send_to_peer(Message::State { monitors: infos });
                }
            }
            NetCmd::LocalSetResult { id, result } => {
                if self.pending_in.remove(&id).is_some() {
                    let (ok, current, error) = match result {
                        Ok(input) => (true, Some(input), None),
                        Err(e) => (false, None, Some(e)),
                    };
                    self.send_to_peer(Message::SwitchResult {
                        id,
                        ok,
                        current,
                        error,
                    });
                }
            }
            NetCmd::RequestSwitch { id, target } => {
                if self.connected() {
                    self.pending_out.insert(id.clone(), Instant::now());
                    self.send_to_peer(Message::SwitchRequest { id, target });
                } else {
                    self.emit(NetEvt::SwitchResult {
                        id,
                        ok: false,
                        error: Some("peer offline".into()),
                    });
                }
            }
        }
    }

    fn handle_candidate(&mut self, c: discovery::Candidate) {
        if self.connected() {
            return;
        }
        // Min-id machine is the connector; the other side waits to accept.
        if self.config.machine_id < c.machine_id {
            let addr = std::net::SocketAddr::new(c.ip, c.tcp_port);
            self.maybe_dial(addr);
        }
    }

    fn handle_new_connection(
        &mut self,
        stream: TcpStream,
        initiated_by_me: bool,
        peer_id: Uuid,
        peer_name: String,
        peer_monitors: Vec<MonitorInfo>,
    ) {
        if !self.should_adopt(peer_id, initiated_by_me) {
            return; // dropping `stream` closes the losing connection
        }

        self.epoch_counter += 1;
        let epoch = self.epoch_counter;
        let out_tx = link::start_io(stream, epoch, self.inbound.clone());
        self.peer = Some(PeerConn {
            epoch,
            peer_id,
            initiated_by_me,
            out_tx,
        });
        self.peer_monitors = peer_monitors;

        self.emit(NetEvt::Status(format!("connected to {peer_name}")));
        self.emit(NetEvt::PeerOnline { name: peer_name });
        self.push_owners();
        let infos = self.build_my_infos();
        self.send_to_peer(Message::State { monitors: infos });
    }

    /// Keep exactly one link per peer, favouring the canonical connector (min-id
    /// dials). A lone non-canonical link is still adopted so we are never left
    /// unconnected when broadcast only works one way.
    fn should_adopt(&self, peer_id: Uuid, initiated_by_me: bool) -> bool {
        let canonical = initiated_by_me == (self.config.machine_id < peer_id);
        match &self.peer {
            None => true,
            Some(current) if current.peer_id == peer_id => {
                let current_canonical =
                    current.initiated_by_me == (self.config.machine_id < peer_id);
                canonical && !current_canonical
            }
            Some(_) => false,
        }
    }

    fn handle_link_message(&mut self, epoch: u64, msg: Message) {
        if self.peer.as_ref().map(|p| p.epoch) != Some(epoch) {
            return; // stale link
        }
        match msg {
            Message::Hello { monitors, .. } | Message::State { monitors } => {
                self.peer_monitors = monitors;
                self.push_owners();
            }
            Message::SwitchRequest { id, target } => {
                if self.owner_of(&id) == Owner::Me {
                    self.pending_in.insert(id.clone(), ());
                    self.emit(NetEvt::ExecuteSwitch { id, target });
                } else {
                    self.send_to_peer(Message::SwitchResult {
                        id,
                        ok: false,
                        current: None,
                        error: Some("not owner".into()),
                    });
                }
            }
            Message::SwitchResult { id, ok, error, .. } => {
                self.pending_out.remove(&id);
                self.emit(NetEvt::SwitchResult { id, ok, error });
            }
            Message::Heartbeat => {}
        }
    }

    fn handle_link_closed(&mut self, epoch: u64) {
        if self.peer.as_ref().map(|p| p.epoch) != Some(epoch) {
            return;
        }
        self.peer = None;
        self.peer_monitors.clear();
        self.last_dial = None; // allow immediate re-dial
        self.emit(NetEvt::Status("peer disconnected".into()));
        self.emit(NetEvt::PeerOffline);
        self.push_owners();
    }

    fn tick(&mut self) {
        let timed_out: Vec<StableId> = self
            .pending_out
            .iter()
            .filter(|(_, t)| t.elapsed() > SWITCH_TIMEOUT)
            .map(|(id, _)| id.clone())
            .collect();
        for id in timed_out {
            self.pending_out.remove(&id);
            self.emit(NetEvt::SwitchResult {
                id,
                ok: false,
                error: Some("peer did not respond".into()),
            });
        }

        if !self.connected()
            && let Some(host) = self.config.peer_host.clone()
        {
            self.dial_host(&host);
        }
    }

    fn maybe_dial(&mut self, addr: std::net::SocketAddr) {
        if self.last_dial.is_some_and(|t| t.elapsed() < DIAL_COOLDOWN) {
            return;
        }
        self.last_dial = Some(Instant::now());
        link::spawn_dial(
            addr,
            self.config.machine_id,
            self.config.machine_name.clone(),
            self.local.clone(),
            self.inbound.clone(),
        );
    }

    fn dial_host(&mut self, host: &str) {
        if self.last_dial.is_some_and(|t| t.elapsed() < DIAL_COOLDOWN) {
            return;
        }
        let target = (host, link::TCP_PORT);
        match target.to_socket_addrs() {
            Ok(mut addrs) => {
                if let Some(addr) = addrs.next() {
                    self.maybe_dial(addr);
                }
            }
            Err(e) => log::debug!("cannot resolve peer_host {host}: {e:#}"),
        }
    }

    /// Learn this machine's home input on each monitor the first time we can read
    /// it while active, then persist. Learned once so a later handoff (which reads
    /// the *peer's* input on our channel) can't overwrite it.
    fn learn_my_inputs(&mut self) {
        let mut changed = false;
        for snap in &self.local_snaps {
            if snap.available
                && let Some(current) = snap.current_input
                && !self.config.my_inputs.contains_key(&snap.id)
            {
                self.config.my_inputs.insert(snap.id.clone(), current);
                changed = true;
            }
        }
        if changed && let Err(e) = self.config.save() {
            log::warn!("failed to persist learned inputs: {e:#}");
        }
    }

    fn build_my_infos(&self) -> Vec<MonitorInfo> {
        self.local_snaps
            .iter()
            .map(|snap| MonitorInfo {
                id: snap.id.clone(),
                my_input: self.config.my_inputs.get(&snap.id).copied(),
                current_input: if snap.available {
                    snap.current_input
                } else {
                    None
                },
                valid_inputs: snap.valid_inputs.clone(),
                label: snap.name.clone(),
            })
            .collect()
    }

    fn owner_of(&self, id: &StableId) -> Owner {
        let local = self.local_snaps.iter().find(|s| s.id == *id);
        let peer = if self.connected() {
            self.peer_monitors.iter().find(|m| m.id == *id)
        } else {
            None
        };
        compute_owner(
            self.config.my_inputs.get(id).copied(),
            local.map(|s| (s.available, s.current_input)),
            peer,
        )
    }

    fn push_owners(&self) {
        let owners = self
            .local_snaps
            .iter()
            .map(|s| (s.id.clone(), self.owner_of(&s.id)))
            .collect();
        self.emit(NetEvt::Owners(owners));
    }
}

/// Ownership from this machine's view. `Me` when we can read our home input on it;
/// otherwise `Peer` when the peer reports it sits on the peer's home input; else
/// `Unknown`.
fn compute_owner(
    my_input: Option<InputSource>,
    local: Option<(bool, Option<InputSource>)>,
    peer: Option<&MonitorInfo>,
) -> Owner {
    if let (Some(mine), Some((true, current))) = (my_input, local)
        && current == Some(mine)
    {
        return Owner::Me;
    }
    if let Some(peer) = peer
        && peer.current_input.is_some()
        && peer.current_input == peer.my_input
    {
        return Owner::Peer;
    }
    Owner::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(model: &str) -> StableId {
        crate::ddc::identity::stable_id(Some("acme"), Some(model), None, Some("serial"), None, 0)
    }

    fn peer_info(my: u8, current: Option<u8>) -> MonitorInfo {
        MonitorInfo {
            id: id("a"),
            my_input: Some(InputSource(my)),
            current_input: current.map(InputSource),
            valid_inputs: vec![],
            label: "A".into(),
        }
    }

    #[test]
    fn i_own_when_i_read_my_input() {
        let owner = compute_owner(
            Some(InputSource(0x11)),
            Some((true, Some(InputSource(0x11)))),
            Some(&peer_info(0x12, None)),
        );
        assert_eq!(owner, Owner::Me);
    }

    #[test]
    fn peer_owns_when_switched_away_and_peer_reads_its_input() {
        let owner = compute_owner(
            Some(InputSource(0x11)),
            Some((false, None)),
            Some(&peer_info(0x12, Some(0x12))),
        );
        assert_eq!(owner, Owner::Peer);
    }

    #[test]
    fn unknown_when_neither_side_reads_its_input() {
        let owner = compute_owner(
            Some(InputSource(0x11)),
            Some((false, None)),
            Some(&peer_info(0x12, None)),
        );
        assert_eq!(owner, Owner::Unknown);
    }

    #[test]
    fn unknown_when_peer_offline_and_i_am_switched_away() {
        let owner = compute_owner(Some(InputSource(0x11)), Some((false, None)), None);
        assert_eq!(owner, Owner::Unknown);
    }

    #[test]
    fn i_do_not_own_a_third_input_i_can_read() {
        let owner = compute_owner(
            Some(InputSource(0x11)),
            Some((true, Some(InputSource(0x0F)))),
            None,
        );
        assert_eq!(owner, Owner::Unknown);
    }

    #[test]
    fn unknown_when_my_input_not_yet_learned() {
        let owner = compute_owner(None, Some((true, Some(InputSource(0x11)))), None);
        assert_eq!(owner, Owner::Unknown);
    }
}
