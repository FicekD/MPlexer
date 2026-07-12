mod discovery;
mod link;
pub mod proto;

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::config::Config;
use crate::ddc::{InputSource, StableId};
use crate::worker::MonitorSnapshot;
use link::LocalView;
pub use link::TCP_PORT;
use proto::{Message, MonitorInfo};

const TICK: Duration = Duration::from_secs(1);
const DIAL_COOLDOWN: Duration = Duration::from_secs(4);
const HOST_OFFLINE_AFTER: Duration = Duration::from_secs(7);
const HOST_DROP_AFTER: Duration = Duration::from_secs(30);

pub enum NetCmd {
    LocalState(Vec<MonitorSnapshot>),
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
    ExecuteSwitch {
        id: StableId,
        target: InputSource,
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
                let infos = self.build_my_infos();
                *self.local.lock().unwrap() = infos.clone();
                if self.connected() {
                    self.send_to_peer(Message::State { monitors: infos });
                }
            }
            NetCmd::RequestSwitch { id, target } => {
                if self.connected() {
                    self.send_to_peer(Message::SwitchRequest { id, target });
                }
            }
            NetCmd::SelectPeer(sel) => self.select_peer(sel),
        }
    }

    fn select_peer(&mut self, sel: Option<Uuid>) {
        match sel {
            Some(id) => log::info!("linking to peer {id}"),
            None => log::info!("unlinking from peer"),
        }
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
        }
    }

    fn handle_candidate(&mut self, c: discovery::Candidate) {
        let addr = SocketAddr::new(c.ip, c.tcp_port);
        if !self.hosts.contains_key(&c.machine_id) {
            log::info!("discovered host '{}' at {}", c.machine_name, addr);
        }
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

        log::info!("connected to peer '{peer_name}' ({peer_id})");
        self.emit(NetEvt::Status(format!("connected to {peer_name}")));
        self.emit(NetEvt::PeerOnline { name: peer_name });
        self.emit_hosts();
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
            }
            Message::SwitchRequest { id, target } => {
                // Execute only if we're cabled to this monitor and can control it.
                if self.local_snaps.iter().any(|s| s.id == id && s.controllable) {
                    log::info!("peer requested switch of {id} to 0x{:02X}", target.0);
                    self.emit(NetEvt::ExecuteSwitch { id, target });
                }
            }
            Message::Heartbeat => {}
        }
    }

    fn handle_link_closed(&mut self, epoch: u64) {
        if self.peer.as_ref().map(|p| p.epoch) != Some(epoch) {
            return;
        }
        log::info!("peer link closed");
        self.peer = None;
        self.peer_monitors.clear();
        self.last_dial = None;
        self.emit(NetEvt::Status("peer disconnected".into()));
        self.emit(NetEvt::PeerOffline);
        self.emit_hosts();
    }

    fn tick(&mut self) {
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

    fn build_my_infos(&self) -> Vec<MonitorInfo> {
        self.local_snaps
            .iter()
            .map(|snap| MonitorInfo {
                id: snap.id.clone(),
                controllable: snap.controllable,
                current_input: if snap.available {
                    snap.current_input
                } else {
                    None
                },
                observed_inputs: snap.observed_inputs.clone(),
                label: snap.name.clone(),
            })
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::MonitorSnapshot;

    fn id(model: &str) -> StableId {
        crate::ddc::identity::stable_id(Some("acme"), Some(model), None, Some("serial"), None, 0)
    }

    fn snap(controllable: bool) -> MonitorSnapshot {
        MonitorSnapshot {
            id: id("a"),
            name: "A".into(),
            controllable,
            current_input: Some(InputSource(0x12)),
            observed_inputs: vec![InputSource(0x12), InputSource(0x0F)],
            available: true,
        }
    }

    #[test]
    fn build_my_infos_carries_controllable_and_observed() {
        let mut net = test_net();
        net.local_snaps = vec![snap(true)];
        let infos = net.build_my_infos();
        assert_eq!(infos.len(), 1);
        assert!(infos[0].controllable);
        assert_eq!(infos[0].observed_inputs, vec![InputSource(0x12), InputSource(0x0F)]);
    }

    fn test_net() -> Net {
        Net {
            config: Config::default(),
            inbound: mpsc::channel().0,
            evt_tx: mpsc::channel().0,
            ctx: egui::Context::default(),
            local: Arc::new(Mutex::new(Vec::new())),
            local_snaps: Vec::new(),
            peer: None,
            peer_monitors: Vec::new(),
            epoch_counter: 0,
            last_dial: None,
            hosts: HashMap::new(),
            last_hosts: Vec::new(),
            selected_peer: None,
        }
    }
}
