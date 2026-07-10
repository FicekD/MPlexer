# Phase 2 — Local UI

**Goal:** an egui desktop window that lists the phase-1 monitors and shows a **dropdown per monitor**
to select its input source, driving real DDC writes — all on a single machine, no networking yet. At
the end of this phase MPlexer is already a usable standalone input switcher.

**Prerequisite:** phase 1 complete (`src/ddc/` working).

## Deliverables

- `src/model.rs` — `AppState`, `MonitorState`, `Owner`.
- `src/worker.rs` — background DDC worker thread (keeps blocking I²C off the UI thread).
- `src/ui/mod.rs` — the `eframe` app.
- `src/main.rs` — launch the GUI by default; keep `--cli` from phase 1.
- `src/config.rs` — minimal now (machine name, input labels); expanded in phase 3.

## Dependencies to add

```bash
cargo add eframe            # pulls egui
cargo add directories       # per-OS config path
cargo add serde --features derive
cargo add serde_json
```

Check the resolved `eframe`/`egui` version on docs.rs — the `App` trait method signature and
`run_native` options struct change between minor versions. Adapt the skeleton below accordingly.

## Threading model (important)

egui runs an immediate-mode loop; **never call DDC from inside `update()`**. Structure:

```
UI thread (eframe)  --WorkerCmd-->  worker thread (owns Vec<ddc::Monitor>)
      ^                                   |
      |------------- WorkerEvt -----------|
        (channel; UI drains it each frame; worker calls ctx.request_repaint())
```

- `WorkerCmd`: `Refresh`, `SetInput { id: StableId, input: InputSource }`.
- `WorkerEvt`: `Monitors(Vec<MonitorSnapshot>)`, `SetResult { id, result: Result<InputSource,String> }`.
- `MonitorSnapshot` is a plain `Clone` data struct (no live handle): `{ id, name, current_input,
  valid_inputs, available: bool }`. The worker owns the non-Clone `ddc::Monitor`s; it sends snapshots.

The worker loop:

1. On start and on `Refresh`, call `ddc::enumerate()` (or re-read existing handles) and emit
   `Monitors(...)`.
2. On `SetInput`, call `write_input`, then **sleep ~1 s**, re-read that monitor, emit `SetResult` and a
   fresh `Monitors(...)`.
3. Poll every ~3 s: re-read inputs, emit `Monitors(...)` only if something changed (avoid needless
   repaints). Pause the poll for ~1 s after a write (step 2 already covers the settle).

Use `std::sync::mpsc` both directions. Clone the `egui::Context` into the worker so it can call
`ctx.request_repaint()` after emitting an event.

## `src/model.rs`

```rust
use crate::ddc::{InputSource, StableId};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner { Me, Peer, Unknown }   // phase 2 is always Me/Unknown; Peer arrives in phase 3

#[derive(Clone, Debug)]
pub struct MonitorState {
    pub id: StableId,
    pub name: String,
    pub current_input: Option<InputSource>,
    pub valid_inputs: Vec<InputSource>,
    pub available: bool,
    pub owner: Owner,
    pub pending: bool,                  // a set is in flight; disable the dropdown, show spinner
}

#[derive(Default)]
pub struct AppState {
    pub monitors: Vec<MonitorState>,
    pub status: String,                // last error / info line shown at the bottom
}
```

Keep `AppState` a plain data holder; the `eframe::App` in `ui/mod.rs` owns it plus the channels.

## `src/config.rs` (minimal for phase 2)

Persisted JSON at `directories::ProjectDirs::from("", "", "MPlexer")` → `config_dir()/config.json`.
Load on start, create with defaults if missing.

```rust
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub machine_name: String,                 // default: hostname
    pub input_labels: std::collections::HashMap<String, String>, // "0x11" -> "Work Laptop"
    // phase 3 will add: machine_id (uuid), peer_host (Option<String>), my_inputs per monitor
}
```

- `input_labels` lets the user rename raw codes to meaningful names in the dropdown (e.g.
  `0x12 → "This PC"`). Merge these over the default MCCS labels when rendering.
- Provide `Config::load()` / `Config::save()` returning `anyhow::Result`. On load error, log and use
  defaults (don't crash).

## `src/ui/mod.rs`

Representative skeleton — adapt to your resolved egui version:

```rust
pub struct MplexerApp {
    state: AppState,
    cmd_tx: std::sync::mpsc::Sender<WorkerCmd>,
    evt_rx: std::sync::mpsc::Receiver<WorkerEvt>,
    config: Config,
}

impl eframe::App for MplexerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. Drain worker events into self.state.
        while let Ok(evt) = self.evt_rx.try_recv() {
            match evt {
                WorkerEvt::Monitors(list) => self.merge_snapshots(list),
                WorkerEvt::SetResult { id, result } => self.apply_set_result(id, result),
            }
        }

        // 2. Render.
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("MPlexer — {}", self.config.machine_name));
            ui.separator();
            for m in &mut self.state.monitors {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(&m.name);
                        if !m.available { ui.label("(unavailable)"); }
                        if m.pending { ui.spinner(); }
                    });

                    let current = m.current_input;
                    let mut selected = current;
                    egui::ComboBox::from_id_salt(&m.id)   // id_salt name varies by version
                        .selected_text(label_for(current, &self.config))
                        .show_ui(ui, |ui| {
                            for &inp in &m.valid_inputs {
                                ui.selectable_value(&mut selected, Some(inp),
                                    label_for(Some(inp), &self.config));
                            }
                        });

                    if selected != current && !m.pending {
                        if let Some(inp) = selected {
                            m.pending = true;
                            self.cmd_tx.send(WorkerCmd::SetInput { id: m.id.clone(), input: inp }).ok();
                        }
                    }
                });
            }
            ui.separator();
            ui.label(&self.state.status);
        });
    }
}
```

- `label_for(Some(input), config)`: config override → else `input.label()`. Include the raw code in the
  text, e.g. `"This PC (HDMI-2)"`, so it's unambiguous.
- `merge_snapshots`: update existing `MonitorState` by `id` (preserve `pending`/`owner`); add new;
  mark missing ones `available = false`. Don't rebuild the vec wholesale (keeps combo state stable).
- `apply_set_result`: clear `pending`; on `Err`, set `state.status` to the error; on `Ok`, update
  `current_input`.
- **Guard against feedback loops:** only send `SetInput` when the change came from the user
  (`selected != current`). When a poll updates `current_input`, that path doesn't send a command.

## `src/main.rs`

- Parse args: if `--cli`, run phase-1 harness and exit; otherwise launch GUI.
- GUI launch: init logging, load config, create the two channels, spawn the worker thread (move the
  `cmd_rx`, `evt_tx`, and a cloned `egui::Context`), then `eframe::run_native(...)`. Obtain the
  `Context` inside the `App` creation closure (`cc.egui_ctx.clone()`), hand it to the worker before or
  as it starts (e.g. send it via a one-shot, or construct the worker inside the closure).

## Acceptance criteria

Automated (here):

- `cargo build`, `cargo clippy --all-targets` clean.
- `cargo test` still green (phase-1 tests; add a `merge_snapshots` unit test if you factor it to be
  testable without egui).

Hardware validation (user runs, supervised — a real switch can blank the screen):

- Launch on Windows: window lists both monitors, dropdown shows `HDMI-1 / HDMI-2 / DisplayPort-1`,
  current = HDMI-2.
- Changing **one** monitor's dropdown to another input switches that monitor; the spinner clears and
  the dropdown reflects the new value; switching back restores it.
- Renaming a code via `config.json` `input_labels` (e.g. `"0x12": "This PC"`) shows the friendly name
  after restart.
- The UI stays responsive during switches (proves DDC isn't on the UI thread).

## Definition of done

A single-machine input switcher with a per-monitor dropdown, non-blocking DDC via the worker thread,
persisted friendly labels, clippy clean.
