# MPlexer — Implementation Overview

MPlexer is a cross-platform (Windows + Linux) desktop app that switches the **input source** of
DDC/CI-capable monitors, and coordinates that switching between two machines that share the same
physical monitors (a software KVM for the *display side* only).

Read this file first. Then implement the phases in order:

1. `01-phase-1-ddc-core.md` — talk to monitors, read/write input source.
2. `02-phase-2-local-ui.md` — egui window with a dropdown per monitor.
3. `03-phase-3-lan-handoff.md` — two machines discover each other and hand off the screens.
4. `04-phase-4-polish.md` — tray icon, run-at-login, hotkey, config. **Deferred; v1 = phases 1–3.**

---

## The problem being solved

Two machines are cabled to the same two monitors, each on a different input:

- **This PC** (dual-boot Windows / Fedora) — e.g. both monitors on **HDMI-2** (`0x12`).
- **Work laptop** (Windows only) — e.g. both monitors on **HDMI-1** (`0x11`) or **DisplayPort-1** (`0x0F`).

The monitors implement the DDC/CI standard: over the video cable's I²C side-channel you can read and
write **VCP feature `0x60` (Input Source Select)** to switch which input a monitor displays. This was
verified on the real hardware — both monitors respond and advertise inputs `0x11`, `0x12`, `0x0F`.

We want an app on each machine that lists the shared monitors and lets the user pick each monitor's
input from a dropdown, with both apps staying in sync.

## Core design decisions (do not re-litigate)

1. **Handoff model, not grab model.**
   A DDC write is *always* performed by the machine that currently **owns** (is the active source of)
   the monitor. A machine never tries to switch a monitor *to itself* directly, because a monitor may
   not answer DDC/CI on a **deselected** input (firmware-dependent, and flaky on NVIDIA-on-Linux).
   Instead, "give me the screen" is a **request sent over the LAN** to the current owner, who executes
   the write. Result: every DDC write is issued by the active source, where DDC always works.

2. **LAN link is the source of truth.**
   The two apps hold a persistent TCP connection and exchange small JSON messages. Ownership and
   current-input state are authoritative over the link, not inferred by polling a monitor you can't
   reach. DDC polling is only a *local confirmation* that the hardware did what was asked.

3. **Auto-discovery with manual fallback.**
   Apps find each other via UDP broadcast on the LAN; a `peer_host` config value forces a direct
   connection when broadcast is unavailable.

4. **Stable monitor identity across machines.**
   OS monitor handles are volatile. Correlate "the same physical panel" across both machines using the
   monitor's EDID-derived identity (manufacturer + model + serial). See phase 1.

## Tech stack

| Concern | Crate | Notes |
|---|---|---|
| DDC/CI | `ddc-hi` (+ `ddc` trait it re-exports) | Windows `dxva2.dll` backend, Linux `/dev/i2c` backend, one API |
| GUI | `eframe` + `egui` | immediate-mode, works on Windows + X11 + Wayland |
| Serialization | `serde`, `serde_json` | LAN messages + config file |
| CLI harness | `clap` (derive) | phase-1 debugging subcommands |
| Logging | `log` + `env_logger` | `RUST_LOG=debug` |
| Config/paths | `directories` | per-OS config dir |
| IDs | `uuid` (v4) | stable per-machine id, persisted in config |
| Channels | `std::sync::mpsc` | UI ⇄ background threads. No `tokio` — use plain threads. |

**IMPORTANT — verify crate APIs before coding.** These crates evolve and their exact signatures may
differ from what this plan shows. Before implementing a phase, run `cargo add <crate>`, note the
resolved version, and confirm the real API on <https://docs.rs/CRATE/VERSION>. The signatures in these
plans are *representative* — adapt to the version you resolve, and prefer the crate's own examples.

## Project layout (target)

Single binary crate (simpler for implementers than a workspace). Build it up module-by-module across
phases:

```
MPlexer/
  Cargo.toml
  plans/                     # these files
  src/
    main.rs                  # arg parse: GUI by default, `--cli <subcommand>` for the phase-1 harness
    ddc/
      mod.rs                 # public API: enumerate(), Monitor, read_input(), write_input()
      identity.rs            # StableId from EDID/model/serial
      inputs.rs              # VCP 0x60 code <-> InputSource, capability-string parsing
    model.rs                 # AppState, MonitorState, Owner
    config.rs                # persisted settings (machine id/name, input labels, peer_host)
    net/                     # phase 3
      mod.rs
      discovery.rs           # UDP broadcast announce/listen
      link.rs                # TCP connect/accept, newline-delimited JSON framing
      proto.rs               # Message enum (serde)
    ui/                      # phase 2
      mod.rs                 # eframe App impl
    worker.rs                # background DDC worker thread (keeps blocking I²C off the UI thread)
    logging.rs               # env_logger init
```

Rename the package to `mplexer` (lowercase) in `Cargo.toml` for a conventional binary name; keep
`edition = "2024"`.

## Cross-cutting conventions

- **Never block the egui update loop on DDC.** DDC/I²C calls take tens of ms and can stall. All DDC
  reads/writes go through `worker.rs` on a dedicated thread; the UI communicates via channels and calls
  `ctx.request_repaint()` when results arrive.
- **Poll gently.** Background input-state polling interval ≈ 3 s. **Pause polling for ~1 s after any
  write** to let the monitor settle and avoid reading a stale value.
- **Fail loudly in logs, degrade gracefully in UI.** A monitor that stops answering DDC should show as
  "unavailable", not crash the app.
- **Follow the user's global style rules:** direct, minimal, production code; split logic across files
  (already reflected in the layout); comment only where the code can't speak for itself; no wrapper
  functions that hold a single expression.

## VCP `0x60` Input Source codes (MCCS 2.2 subset)

Used by `ddc/inputs.rs`. Note some monitors use **vendor-specific** codes (e.g. USB-C reported as a DP
or vendor value) — always cross-check against the monitor's advertised capability list, don't assume.

| Code | Input          | Code | Input           |
|------|----------------|------|-----------------|
| 0x01 | VGA-1          | 0x0F | DisplayPort-1   |
| 0x03 | DVI-1          | 0x10 | DisplayPort-2   |
| 0x04 | DVI-2          | 0x11 | HDMI-1          |
| 0x0C | Component-1    | 0x12 | HDMI-2          |

Unknown codes must be preserved and shown as `Input 0xNN` rather than dropped.

## What can and cannot be tested here vs. by the user

- **Buildable/unit-testable anywhere:** input-code mapping, capability-string parsing, protocol
  serde round-trips, discovery message encode/decode, UI compiles and renders.
- **Needs the real hardware (user runs it):** any DDC read/write against monitors, the Fedora/NVIDIA
  I²C path, and the actual two-machine handoff. Each phase lists explicit commands for the user to run
  and the expected output. Do **not** perform a real input switch unattended — it can blank the
  operator's screen (see the handoff rationale above).

## Definition of done (v1)

On both machines, launching MPlexer shows the two shared monitors, each with a working input dropdown;
selecting the other machine's input hands the screen over reliably; each app reflects switches made
from the other machine within a couple of seconds. Phases 1–3 complete, `cargo clippy` clean,
unit tests pass.
