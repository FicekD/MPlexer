use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::NetInbound;

pub const UDP_PORT: u16 = 48619;
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(3);

pub struct Candidate {
    pub ip: IpAddr,
    pub tcp_port: u16,
    pub machine_id: Uuid,
    pub machine_name: String,
}

#[derive(Serialize, Deserialize)]
struct Announce {
    machine_id: Uuid,
    machine_name: String,
    tcp_port: u16,
}

pub fn spawn(machine_id: Uuid, machine_name: String, tcp_port: u16, inbound: Sender<NetInbound>) {
    let socket = match UdpSocket::bind(("0.0.0.0", UDP_PORT)) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("discovery disabled: cannot bind UDP {UDP_PORT}: {e:#}");
            return;
        }
    };
    if let Err(e) = socket.set_broadcast(true) {
        log::warn!("discovery disabled: cannot enable broadcast: {e:#}");
        return;
    }

    let listen_socket = match socket.try_clone() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("discovery disabled: cannot clone socket: {e:#}");
            return;
        }
    };

    thread::spawn(move || announce(&socket, machine_id, &machine_name, tcp_port));
    thread::spawn(move || listen(&listen_socket, machine_id, &inbound));
}

fn announce(socket: &UdpSocket, machine_id: Uuid, machine_name: &str, tcp_port: u16) {
    let payload = match serde_json::to_vec(&Announce {
        machine_id,
        machine_name: machine_name.to_string(),
        tcp_port,
    }) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::error!("cannot encode announce: {e:#}");
            return;
        }
    };
    loop {
        for target in broadcast_targets() {
            if let Err(e) = socket.send_to(&payload, (target, UDP_PORT)) {
                log::debug!("announce broadcast to {target} failed: {e:#}");
            }
        }
        thread::sleep(ANNOUNCE_INTERVAL);
    }
}

/// Windows sends 255.255.255.255 out of a single adapter, often a virtual one,
/// so announce on every IPv4 subnet's own broadcast address instead. Interfaces
/// are listed afresh each round so adapters that come up later are covered.
fn broadcast_targets() -> Vec<Ipv4Addr> {
    let mut targets: Vec<Ipv4Addr> = match if_addrs::get_if_addrs() {
        Ok(ifaces) => ifaces
            .into_iter()
            .filter(|iface| iface.is_oper_up())
            .filter_map(|iface| match iface.addr {
                if_addrs::IfAddr::V4(v4) if !v4.ip.is_loopback() => {
                    Some(v4.broadcast.unwrap_or_else(|| {
                        Ipv4Addr::from(u32::from(v4.ip) | !u32::from(v4.netmask))
                    }))
                }
                _ => None,
            })
            .collect(),
        Err(e) => {
            log::debug!("cannot list network interfaces: {e:#}");
            Vec::new()
        }
    };
    targets.sort();
    targets.dedup();
    if targets.is_empty() {
        targets.push(Ipv4Addr::BROADCAST);
    }
    targets
}

fn listen(socket: &UdpSocket, my_id: Uuid, inbound: &Sender<NetInbound>) {
    let mut buf = [0u8; 512];
    loop {
        let (len, addr) = match socket.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e) => {
                log::debug!("discovery recv failed: {e:#}");
                continue;
            }
        };
        let announce: Announce = match serde_json::from_slice(&buf[..len]) {
            Ok(a) => a,
            Err(_) => continue,
        };
        if announce.machine_id == my_id {
            continue;
        }
        let candidate = Candidate {
            ip: addr.ip(),
            tcp_port: announce.tcp_port,
            machine_id: announce.machine_id,
            machine_name: announce.machine_name,
        };
        if inbound.send(NetInbound::Candidate(candidate)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_round_trips() {
        let a = Announce {
            machine_id: Uuid::new_v4(),
            machine_name: "laptop".into(),
            tcp_port: 48620,
        };
        let bytes = serde_json::to_vec(&a).unwrap();
        let back: Announce = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(a.machine_id, back.machine_id);
        assert_eq!(a.machine_name, back.machine_name);
        assert_eq!(a.tcp_port, back.tcp_port);
    }
}
