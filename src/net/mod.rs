mod discovery;
mod link;
pub mod proto;

use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::config::Config;
use crate::ddc::{InputSource, StableId};
use crate::model::Owner;
use crate::worker::MonitorSnapshot;
use link::LocalView;
pub use link::TCP_PORT;
use proto::{Message, MonitorInfo};

const TICK: Duration = Duration::from_secs(1);
const DIAL_COOLDOWN: Duration = Duration::from_secs(4);
const SWITCH_TIMEOUT: Duration = Duration::from_secs(8);
const HOST_OFFLINE_AFTER: Duration = Duration::from_secs(7);
const HOST_DROP_AFTER: Duration = Duration::from_secs(30);

pub enum NetCmd {
    LocalState(Vec<MonitorSnapshot>),
    LocalSetResult {
        id: StableId,
        result: Result<InputSource, String>,
    },
    RequestSwitch { id: StableId, target: InputSource },
    SelectPeer(Option<Uuid>),
}

#[derive(Clone, PartialEq)]
pub struct HostView {
    pub name: String,
    pub addr: String,
    pub machine_id: Uuid,
    pub status: HostStatus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HostStatus {
    Connected,
    Connecting,
    Online,
    Offline,
}

pub enum NetEvt {
    PeerOnline {
        name: String,
    },
    PeerOffline,
    Hosts(Vec<HostView>),
    Owners(HashMap<StableId, Owner>),
    ExecuteSwitch {
        id: StableId,
        target: InputSource,
    },
    SwitchResult {
        id: StableId,
        ok: bool,
        error: Option<String>,
    },
    Status(String),
}

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

struct HostRecord {
    name: String,
    sock: Option<SocketAddr>,
    last_seen: Instant,
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
    pending_out: HashMap<StableId, Instant>,
    pending_in: HashSet<StableId>,
    last_dial: Option<Instant>,
    hosts: HashMap<Uuid, HostRecord>,
    last_hosts: Vec<HostView>,
    selected_peer: Option<Uuid>,
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
        config.comms_port,
        in_tx.clone(),
    );
    link::spawn_accept(
        config.machine_id,
        config.machine_name.clone(),
        config.comms_port,
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
        pending_in: HashSet::new(),
        last_dial: None,
        hosts: HashMap::new(),
        last_hosts: Vec::new(),
        selected_peer: None,
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
                if self.pending_in.remove(&id) {
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
            NetCmd::SelectPeer(sel) => self.select_peer(sel),
        }
    }

    fn select_peer(&mut self, sel: Option<Uuid>) {
        self.selected_peer = sel;
        if self.peer.as_ref().is_some_and(|p| Some(p.peer_id) != sel) {
            self.disconnect();
        }
        if let Some(id) = sel
            && !self.connected()
            && let Some(sock) = self.hosts.get(&id).and_then(|h| h.sock)
        {
            self.last_dial = None;
            self.maybe_dial(sock);
        }
        self.emit_hosts();
    }

    fn disconnect(&mut self) {
        if self.peer.take().is_some() {
            self.peer_monitors.clear();
            self.last_dial = None;
            self.emit(NetEvt::Status("disconnected".into()));
            self.emit(NetEvt::PeerOffline);
            self.push_owners();
        }
    }

    fn handle_candidate(&mut self, c: discovery::Candidate) {
        let addr = SocketAddr::new(c.ip, c.tcp_port);
        self.hosts.insert(
            c.machine_id,
            HostRecord {
                name: c.machine_name,
                sock: Some(addr),
                last_seen: Instant::now(),
            },
        );
        self.emit_hosts();

        if !self.connected() && self.selected_peer == Some(c.machine_id) {
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
        if self.selected_peer.is_some_and(|sel| sel != peer_id) {
            return;
        }
        if !self.should_adopt(peer_id, initiated_by_me) {
            return; // dropping `stream` closes the losing connection
        }

        self.epoch_counter += 1;
        let epoch = self.epoch_counter;
        let peer_addr = stream.peer_addr().ok();
        let out_tx = link::start_io(stream, epoch, self.inbound.clone());
        self.peer = Some(PeerConn {
            epoch,
            peer_id,
            initiated_by_me,
            out_tx,
        });
        self.peer_monitors = peer_monitors;
        self.hosts.insert(
            peer_id,
            HostRecord {
                name: peer_name.clone(),
                sock: peer_addr,
                last_seen: Instant::now(),
            },
        );

        self.emit(NetEvt::Status(format!("connected to {peer_name}")));
        self.emit(NetEvt::PeerOnline { name: peer_name });
        self.emit_hosts();
        self.push_owners();
        let infos = self.build_my_infos();
        self.send_to_peer(Message::State { monitors: infos });
    }

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
            return;
        }
        match msg {
            Message::Hello { monitors, .. } | Message::State { monitors } => {
                self.peer_monitors = monitors;
                self.push_owners();
            }
            Message::SwitchRequest { id, target } => {
                if self.owner_of(&id) == Owner::Me {
                    self.pending_in.insert(id.clone());
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
        self.last_dial = None;
        self.emit(NetEvt::Status("peer disconnected".into()));
        self.emit(NetEvt::PeerOffline);
        self.push_owners();
        self.emit_hosts();
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
            && let Some(id) = self.selected_peer
            && let Some(sock) = self.hosts.get(&id).and_then(|h| h.sock)
        {
            self.maybe_dial(sock);
        }

        let peer_id = self.peer.as_ref().map(|p| p.peer_id);
        self.hosts
            .retain(|id, h| Some(*id) == peer_id || h.last_seen.elapsed() < HOST_DROP_AFTER);
        self.emit_hosts();
    }

    fn maybe_dial(&mut self, addr: SocketAddr) {
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

    fn emit_hosts(&mut self) {
        let peer_id = self.peer.as_ref().map(|p| p.peer_id);
        let mut view: Vec<HostView> = self
            .hosts
            .iter()
            .map(|(id, h)| {
                let status = if Some(*id) == peer_id {
                    HostStatus::Connected
                } else if self.selected_peer == Some(*id) {
                    HostStatus::Connecting
                } else if h.last_seen.elapsed() < HOST_OFFLINE_AFTER {
                    HostStatus::Online
                } else {
                    HostStatus::Offline
                };
                HostView {
                    name: h.name.clone(),
                    addr: h.sock.map(|s| s.to_string()).unwrap_or_default(),
                    machine_id: *id,
                    status,
                }
            })
            .collect();
        view.sort_by(|a, b| a.name.cmp(&b.name).then(a.machine_id.cmp(&b.machine_id)));

        if view != self.last_hosts {
            self.last_hosts = view.clone();
            self.emit(NetEvt::Hosts(view));
        }
    }
}

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
    // A display we can see but can't read the input on (e.g. a TV that doesn't
    // report VCP 0x60) is assumed ours, since the peer above isn't showing it.
    if let Some((true, None)) = local {
        return Owner::Me;
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

    #[test]
    fn present_unreadable_display_is_mine_when_peer_not_showing_it() {
        // A TV we can see but can't read the input on, with no peer claim.
        let owner = compute_owner(None, Some((true, None)), None);
        assert_eq!(owner, Owner::Me);
    }

    #[test]
    fn present_unreadable_display_yields_to_peer_that_shows_it() {
        let owner = compute_owner(None, Some((true, None)), Some(&peer_info(0x12, Some(0x12))));
        assert_eq!(owner, Owner::Peer);
    }
}
