# Phase 3 — LAN Discovery & Handoff

**Goal:** the two machines find each other on the LAN, share monitor state, and hand the screens back
and forth using the **handoff model** — the machine that currently owns a monitor performs the DDC
write on request from the peer. After this phase, MPlexer is the real two-machine tool.

**Prerequisite:** phases 1–2 complete.

## Deliverables

- `src/net/proto.rs` — wire messages (serde).
- `src/net/discovery.rs` — UDP broadcast announce + listen.
- `src/net/link.rs` — TCP connect/accept + newline-delimited JSON framing + read/write loops.
- `src/net/mod.rs` — the net task that ties discovery + link together and bridges to the UI/worker via
  channels.
- Extend `src/config.rs`: `machine_id: Uuid`, `peer_host: Option<String>`, and per-monitor
  `my_inputs: HashMap<StableId, InputSource>` (which input *this* machine occupies on each monitor).
- Extend `src/model.rs`: `Owner::Peer` is now real; add peer connection status.
- Wire the net task into `main.rs` and `ui/mod.rs`.

## Dependencies to add

```bash
cargo add uuid --features v4,serde
# serde / serde_json already present from phase 2
```

No `tokio`. Use blocking `std::net` sockets on dedicated threads; bridge to the UI with `mpsc` and
`ctx.request_repaint()`, exactly like the phase-2 worker.

## Key concept: "my input" per monitor

Each machine occupies a specific input on each monitor (this PC = HDMI-2, laptop = HDMI-1, say). A
machine learns its own input by reading VCP `0x60` **while it is the active source** (which it is,
since the user can see the app). At startup, for each enumerated monitor, if `current_input` is read
successfully, record it as `config.my_inputs[id]` (persist it — so we still know "my input" even later
when we've been switched away and can't read).

**Owner determination:** for a monitor, `Owner::Me` iff its current input == `my_inputs[id]`. If we
can't read it (switched away) we rely on the peer's reported state. Combine both machines' views:

- If I read current == my input → I own it (`Me`), tell peer.
- Peer does the same from its side. Since only one input is physically active at a time, at most one
  machine claims ownership; the other shows `Peer`. If neither can read / neither claims → `Unknown`.

## `src/net/proto.rs`

```rust
use crate::ddc::{InputSource, StableId};
use serde::{Serialize, Deserialize};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MonitorInfo {
    pub id: StableId,
    pub my_input: Option<InputSource>,   // the sender's own input on this monitor
    pub current_input: Option<InputSource>, // what the sender currently reads (None if unreachable)
    pub valid_inputs: Vec<InputSource>,
    pub label: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum Message {
    /// First message after TCP connect. Identity + full monitor view.
    Hello { machine_id: Uuid, machine_name: String, monitors: Vec<MonitorInfo> },
    /// Periodic / on-change full state push (authoritative from the sender's side).
    State { monitors: Vec<MonitorInfo> },
    /// "Please switch monitor <id> to <target>." Routed to the current owner, who executes it.
    SwitchRequest { id: StableId, target: InputSource },
    /// Result of a SwitchRequest the receiver executed.
    SwitchResult { id: StableId, ok: bool, current: Option<InputSource>, error: Option<String> },
    Heartbeat,
}
```

Serialize `InputSource`/`StableId` with serde (derive on the phase-1 types — add
`#[derive(Serialize, Deserialize)]`; `InputSource` is a `u8` newtype, `StableId` a `String` newtype).

**Unit test:** every `Message` variant round-trips through `serde_json::to_string` /`from_str`.

## `src/net/link.rs` — framing & connection

- **Framing:** one JSON `Message` per line, `\n`-terminated (`serde_json::to_string` never emits raw
  newlines). Read with `BufReader::read_line`. Reject lines over a sane cap (e.g. 64 KiB).
- **Roles:** both machines `TcpListener::bind` on the app port (pick one, e.g. `48620`) *and* attempt
  to connect to a discovered/configured peer. To avoid a double connection, keep the link where
  `min(my_machine_id, peer_machine_id)` is the **connector**; if a second socket appears, drop the one
  that violates that rule. (Simplest robust tie-break.)
- Two threads per live connection: a **reader** (`read_line` → parse → forward `Message` to the net
  task via channel) and a **writer** (owns the `TcpStream` write half, drains an outbound `mpsc`). A
  `Heartbeat` every ~5 s; if no line received for ~15 s, drop the connection and go back to
  discovering.
- On any I/O error, close both halves, notify the net task (`PeerDisconnected`), and let discovery
  re-establish.

## `src/net/discovery.rs` — UDP broadcast

- Bind a UDP socket to `0.0.0.0:48619`, enable `SO_BROADCAST` and `SO_REUSEADDR`.
- **Announce** every ~3 s: broadcast (`255.255.255.255:48619`) a small JSON
  `{ machine_id, machine_name, tcp_port }`.
- **Listen:** on receiving an announce from a *different* `machine_id`, hand `(peer_addr_ip, tcp_port,
  machine_id)` to the net task, which decides whether to connect (per the min-id connector rule) if not
  already linked.
- **Manual fallback:** if `config.peer_host` is set, skip waiting for broadcast and connect directly to
  `peer_host:48620` (still honor the connector tie-break once `Hello` exchanges ids).
- Guard against self-receipt (ignore announces whose `machine_id == mine`). Note: on some networks
  broadcast is filtered — that's exactly why `peer_host` exists.

## `src/net/mod.rs` — the net task

Owns the coordination. Runs on its own thread; communicates with the UI/worker via channels
(`NetCmd` in, `NetEvt` out), mirroring phase 2.

State it maintains: `peer: Option<PeerConn>`, latest `peer_monitors: Vec<MonitorInfo>`, and a clone of
the local monitor view (fed from the worker's snapshots).

Behavior:

- **Local state changes** (from worker snapshots via `NetCmd::LocalState`) → recompute my
  `MonitorInfo` list → if connected, send `Message::State`.
- **`Message::Hello` / `State` from peer** → store `peer_monitors`; recompute `Owner` per monitor
  (Me/Peer/Unknown) using the "my input" rule above; emit `NetEvt::PeerState { monitors_with_owner,
  peer_name }` to the UI.
- **User picks an input in the UI for a monitor I don't own** → UI sends `NetCmd::RequestSwitch { id,
  target }` → net task sends `Message::SwitchRequest` to peer.
- **`Message::SwitchRequest` received** → only act if *I* currently own that monitor (I can read it and
  current == my_input). Forward to the worker as a normal `SetInput`; when the worker returns
  `SetResult`, reply `Message::SwitchResult`. If I don't own it, reply `ok=false, error="not owner"`.
- **`Message::SwitchResult` received** → clear the monitor's `pending`, update state, surface any error
  in the status line.
- **`PeerDisconnected`** → mark peer offline in the UI; monitors the peer owned become `Unknown`.

### UI decision logic (where a dropdown change routes)

When the user selects `target` for monitor `id`:

1. If `owner == Me` → local `WorkerCmd::SetInput` (phase-2 path). After it completes we'll have handed
   the screen to the peer; the subsequent `State` exchange updates both apps.
2. If `owner == Peer` → `NetCmd::RequestSwitch { id, target }`. Set `pending`; clear on `SwitchResult`
   or a timeout (~8 s → show "peer did not respond").
3. If `owner == Unknown` and peer offline → attempt a local write anyway *only if* we can read the
   monitor (we're likely the owner); otherwise disable the dropdown with a tooltip "peer offline".

## Config additions (`src/config.rs`)

```rust
pub machine_id: Uuid,                                   // generated once, persisted
pub peer_host: Option<String>,                          // manual fallback, e.g. "192.168.1.50"
pub my_inputs: std::collections::HashMap<StableId, InputSource>, // learned at startup, persisted
```

Generate `machine_id` with `Uuid::new_v4()` on first run and save immediately.

## Ports (document in README later)

- UDP discovery: `48619`
- TCP link: `48620`

Both must be allowed through the OS firewall on the LAN. On Windows the first bind will prompt; on
Fedora add `firewall-cmd` rules. Note this in the completion write-up.

## Acceptance criteria

Automated (here):

- `cargo build`, `cargo clippy --all-targets` clean.
- `cargo test`: `proto` round-trip tests; a discovery encode/decode test; an owner-computation unit
  test (given my_inputs + local reads + peer MonitorInfo → expected `Owner`).

Two-machine validation (user runs on the real machines):

1. Launch on both (same LAN). Within a few seconds each shows the other as **connected** (peer name in
   the header/status).
2. Both list the same two monitors (matching `StableId`s), each showing correct `Owner` (the active
   machine = `Me`, the other = `Peer`).
3. From the **laptop**, pick "This PC → laptop input" for both monitors → screens hand over to the
   laptop; the PC app updates to show `Owner::Peer` for those monitors within ~1 poll cycle.
4. From the **PC** (now the active side after switching back, or via request), hand them back. Verify
   the reverse.
5. Kill one app → the other shows peer offline; relaunch → they re-discover and re-sync.
6. Manual fallback: set `peer_host` on both, disable broadcast (or test across subnets) → still
   connects.

## Definition of done

Two machines auto-discover, stay in sync, and reliably hand off both monitors via the handoff model
(writes always issued by the current owner); peer offline/reconnect handled; clippy clean; unit tests
green.
