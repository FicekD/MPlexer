# Phase 4 — Polish (deferred; not part of v1)

**Goal:** make MPlexer pleasant to live with — system tray, run-at-login, a global hotkey to toggle the
screens, and config UI. **Deferred:** do not start until phases 1–3 are validated on the real machines.
Each item here is independent; implement in any order and ship incrementally.

**Prerequisite:** phases 1–3 complete and working two-machine.

## Platform reality check (read first)

The core app is fully cross-platform, but these niceties are **not equally available** on both OSes:

| Feature | Windows | Linux X11 | Linux Wayland |
|---|---|---|---|
| Tray icon | ✅ | ✅ | ⚠️ GNOME needs AppIndicator extension |
| Run at login | ✅ (registry / Startup) | ✅ (`.desktop` autostart) | ✅ (`.desktop` autostart) |
| Global hotkey | ✅ | ✅ | ❌ not portable (compositor blocks it) |

Gate each feature behind capability detection and degrade gracefully — never hard-fail on a platform
that can't do it. On Wayland, silently skip global-hotkey registration and log an info line.

## 4a — System tray

```bash
cargo add tray-icon
```

- Minimize-to-tray: intercept the window close so it hides instead of exits (keep the net link alive in
  the background). Tray menu: **Show window**, **Switch screens to me / release to peer** (a one-click
  handoff of *all* shared monitors), **Quit**.
- On Linux the `tray-icon` backend uses the StatusNotifier/AppIndicator protocol; document that GNOME
  users must enable the AppIndicator extension. If tray init fails, keep running as a plain window.
- The tray event loop and egui event loop must cooperate — check the `tray-icon` docs for the
  eframe/winit integration pattern for your versions (often: build the tray with the same event loop,
  or run tray on its own thread and bridge via channel).

## 4b — Run at login

- **Windows:** add a value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` pointing at the
  exe (use the `winreg` crate), toggled by a settings checkbox. Prefer HKCU (no admin needed).
- **Linux:** write `~/.config/autostart/mplexer.desktop` with `Exec=` the binary path and
  `X-GNOME-Autostart-enabled=true`. Remove the file to disable.
- Add `start_at_login: bool` to `Config`; apply on save.

## 4c — Global hotkey (Windows + X11 only)

```bash
cargo add global-hotkey
```

- One toggle hotkey (default e.g. `Ctrl+Alt+F12`, configurable): "hand all shared monitors to me" if I
  don't own them, else "release to peer". This is the killer feature for your use case — press it on
  whichever keyboard is active.
- Register at startup; on Wayland, detect and skip with an info log + a disabled/greyed setting in the
  UI explaining why.
- Route the hotkey action through the same handoff logic as the tray "switch to me" item.

## 4d — Config / settings UI

- An egui settings panel (or window) exposing: `machine_name`, per-input friendly labels
  (`input_labels`), `peer_host`, `start_at_login`, hotkey binding.
- Editing labels updates the dropdowns live; editing `peer_host` triggers a reconnect.
- Persist via the existing `Config::save()`.

## 4e — Packaging & docs

- A `README.md`: what it is, the DDC/CI requirement, the two ports (UDP 48619 / TCP 48620) and firewall
  notes, and the **Linux setup steps** (`modprobe i2c-dev`, `i2c` group, NVIDIA caveat, GNOME
  AppIndicator extension).
- Build release binaries per OS: `cargo build --release` on each (or cross-compile). Two artifacts: a
  Windows `.exe` and a Linux ELF. No installer needed for v1 — copy the binary.
- Optional: a small `mplexer.desktop` shipped for Linux menu integration.

## Acceptance criteria

- Tray: closing hides to tray on all three environments (or cleanly falls back to a window where tray
  is unavailable); tray "switch to me" hands all monitors over.
- Run-at-login toggles the registry value / autostart file and actually launches on next login.
- Global hotkey toggles ownership on Windows and X11; on Wayland it's absent and the UI says why.
- Settings changes persist and take effect without a restart where reasonable.
- README lets a fresh user set it up on both machines from scratch.

## Definition of done

MPlexer runs quietly in the tray, starts with the session, and a single hotkey/tray-click hands the
shared monitors between the two machines — with every platform limitation handled gracefully rather
than crashing.
