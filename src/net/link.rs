use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use uuid::Uuid;

use super::NetInbound;
use super::proto::{Message, MonitorInfo};

pub const TCP_PORT: u16 = 48620;
const MAX_LINE: usize = 64 * 1024;
const HEARTBEAT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Snapshot of the local monitor view, shared with dial/accept threads so the
/// `Hello` handshake carries our current state.
pub type LocalView = Arc<Mutex<Vec<MonitorInfo>>>;

/// Accept incoming peer connections forever, running each handshake on its own thread.
pub fn spawn_accept(
    my_id: Uuid,
    my_name: String,
    port: u16,
    local: LocalView,
    inbound: Sender<NetInbound>,
) {
    thread::spawn(move || {
        let listener = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(e) => {
                log::error!("cannot bind TCP {port}: {e:#}");
                return;
            }
        };
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let my_name = my_name.clone();
                    let local = local.clone();
                    let inbound = inbound.clone();
                    thread::spawn(move || {
                        run_handshake(stream, false, my_id, &my_name, &local, &inbound);
                    });
                }
                Err(e) => log::debug!("accept failed: {e:#}"),
            }
        }
    });
}

/// Dial a peer and run the handshake on its own thread.
pub fn spawn_dial(
    addr: SocketAddr,
    my_id: Uuid,
    my_name: String,
    local: LocalView,
    inbound: Sender<NetInbound>,
) {
    thread::spawn(move || {
        let stream = match TcpStream::connect(addr) {
            Ok(s) => s,
            Err(e) => {
                log::debug!("dial {addr} failed: {e:#}");
                return;
            }
        };
        run_handshake(stream, true, my_id, &my_name, &local, &inbound);
    });
}

fn run_handshake(
    mut stream: TcpStream,
    initiated_by_me: bool,
    my_id: Uuid,
    my_name: &str,
    local: &LocalView,
    inbound: &Sender<NetInbound>,
) {
    match handshake(&mut stream, my_id, my_name, local) {
        Ok((peer_id, peer_name, peer_monitors)) => {
            let _ = inbound.send(NetInbound::NewConnection {
                stream,
                initiated_by_me,
                peer_id,
                peer_name,
                peer_monitors,
            });
        }
        Err(e) => log::debug!("handshake failed: {e:#}"),
    }
}

/// Exchange `Hello`s: send ours, read theirs. Reads exactly one line so no bytes
/// are buffered past the handshake for the reader loop.
fn handshake(
    stream: &mut TcpStream,
    my_id: Uuid,
    my_name: &str,
    local: &LocalView,
) -> io::Result<(Uuid, String, Vec<MonitorInfo>)> {
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;

    let hello = Message::Hello {
        machine_id: my_id,
        machine_name: my_name.to_string(),
        monitors: local.lock().unwrap().clone(),
    };
    write_message(stream, &hello)?;

    let line = read_line_raw(stream, MAX_LINE)?;
    let msg: Message = serde_json::from_str(&line)
        .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
    match msg {
        Message::Hello {
            machine_id,
            machine_name,
            monitors,
        } => Ok((machine_id, machine_name, monitors)),
        _ => Err(io::Error::new(
            ErrorKind::InvalidData,
            "expected Hello as first message",
        )),
    }
}

/// Adopt an established connection: spawn reader + writer loops bound to `epoch`,
/// returning the sender used to push messages to the peer.
pub fn start_io(stream: TcpStream, epoch: u64, inbound: Sender<NetInbound>) -> Sender<Message> {
    stream.set_read_timeout(Some(READ_TIMEOUT)).ok();
    let read_stream = stream
        .try_clone()
        .expect("clone TcpStream for reader");
    let write_stream = stream;

    let (out_tx, out_rx) = mpsc::channel::<Message>();
    thread::spawn(move || reader_loop(read_stream, epoch, &inbound));
    thread::spawn(move || writer_loop(write_stream, &out_rx));
    out_tx
}

fn reader_loop(stream: TcpStream, epoch: u64, inbound: &Sender<NetInbound>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if line.len() > MAX_LINE {
                    log::warn!("dropping over-long line from peer");
                    break;
                }
                let trimmed = line.trim_end();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Message>(trimmed) {
                    Ok(Message::Heartbeat) => continue,
                    Ok(msg) => {
                        if inbound.send(NetInbound::LinkMessage { epoch, msg }).is_err() {
                            return;
                        }
                    }
                    Err(e) => log::warn!("ignoring malformed peer message: {e:#}"),
                }
            }
            Err(e) => {
                log::debug!("peer read ended: {e:#}");
                break;
            }
        }
    }
    let _ = inbound.send(NetInbound::LinkClosed { epoch });
}

fn writer_loop(mut stream: TcpStream, out_rx: &mpsc::Receiver<Message>) {
    loop {
        let msg = match out_rx.recv_timeout(HEARTBEAT) {
            Ok(msg) => msg,
            Err(RecvTimeoutError::Timeout) => Message::Heartbeat,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if let Err(e) = write_message(&mut stream, &msg) {
            log::debug!("peer write ended: {e:#}");
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn write_message(stream: &mut TcpStream, msg: &Message) -> io::Result<()> {
    let mut line = serde_json::to_string(msg).map_err(io::Error::other)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()
}

/// Read one `\n`-terminated line without buffering past it (used only during the
/// handshake, so the follow-on reader keeps a clean stream).
fn read_line_raw(stream: &mut TcpStream, max: usize) -> io::Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if stream.read(&mut byte)? == 0 {
            return Err(io::Error::new(ErrorKind::UnexpectedEof, "eof during handshake"));
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > max {
            return Err(io::Error::new(ErrorKind::InvalidData, "handshake line too long"));
        }
    }
    String::from_utf8(buf).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}
