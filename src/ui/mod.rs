use std::sync::mpsc::{Receiver, Sender};

use crate::config::Config;
use crate::ddc::{InputSource, StableId};
use crate::model::{AppState, MonitorState, Owner};
use crate::worker::{MonitorSnapshot, WorkerCmd, WorkerEvt};

pub struct MplexerApp {
    state: AppState,
    cmd_tx: Sender<WorkerCmd>,
    evt_rx: Receiver<WorkerEvt>,
    config: Config,
}

impl MplexerApp {
    pub fn new(config: Config, cmd_tx: Sender<WorkerCmd>, evt_rx: Receiver<WorkerEvt>) -> Self {
        Self {
            state: AppState::default(),
            cmd_tx,
            evt_rx,
            config,
        }
    }
}

impl eframe::App for MplexerApp {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(evt) = self.evt_rx.try_recv() {
            match evt {
                WorkerEvt::Monitors(list) => merge_snapshots(&mut self.state.monitors, list),
                WorkerEvt::SetResult { id, result } => {
                    apply_set_result(&mut self.state, id, result)
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("MPlexer \u{2014} {}", self.config.machine_name));
                if ui.button("Refresh").clicked() {
                    self.cmd_tx.send(WorkerCmd::Refresh).ok();
                }
            });
            ui.separator();

            for m in &mut self.state.monitors {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(&m.name);
                        if !m.available {
                            ui.label("(unavailable)");
                        }
                        if m.owner != Owner::Me {
                            ui.label(format!("({:?})", m.owner));
                        }
                        if m.pending {
                            ui.spinner();
                        }
                    });

                    let current = m.current_input;
                    let mut selected = current;
                    egui::ComboBox::from_id_salt(&m.id)
                        .selected_text(label_for(current, &self.config))
                        .show_ui(ui, |ui| {
                            for &inp in &m.valid_inputs {
                                ui.selectable_value(
                                    &mut selected,
                                    Some(inp),
                                    label_for(Some(inp), &self.config),
                                );
                            }
                        });

                    if selected != current
                        && !m.pending
                        && let Some(inp) = selected
                    {
                        m.pending = true;
                        self.cmd_tx
                            .send(WorkerCmd::SetInput {
                                id: m.id.clone(),
                                input: inp,
                            })
                            .ok();
                    }
                });
            }

            ui.separator();
            ui.label(&self.state.status);
        });
    }
}

fn label_for(input: Option<InputSource>, config: &Config) -> String {
    let Some(input) = input else {
        return "unknown".to_string();
    };
    let code = format!("0x{:02X}", input.0);
    match config.input_labels.get(&code) {
        Some(custom) => format!("{custom} ({})", input.label()),
        None => input.label(),
    }
}

/// Update `existing` in place from a fresh worker snapshot: matched monitors
/// keep their `pending`/`owner` fields, unmatched incoming ones are appended,
/// and existing ones missing from `incoming` are marked unavailable. Never
/// rebuilds the vec wholesale so egui's combo-box widget state stays stable.
fn merge_snapshots(existing: &mut Vec<MonitorState>, incoming: Vec<MonitorSnapshot>) {
    for snap in &incoming {
        if let Some(m) = existing.iter_mut().find(|m| m.id == snap.id) {
            m.name = snap.name.clone();
            m.current_input = snap.current_input;
            m.valid_inputs = snap.valid_inputs.clone();
            m.available = snap.available;
        } else {
            existing.push(MonitorState {
                id: snap.id.clone(),
                name: snap.name.clone(),
                current_input: snap.current_input,
                valid_inputs: snap.valid_inputs.clone(),
                available: snap.available,
                owner: Owner::Me, // phase 2 is single-machine: every enumerated monitor is ours.
                pending: false,
            });
        }
    }
    for m in existing.iter_mut() {
        if !incoming.iter().any(|s| s.id == m.id) {
            m.available = false;
        }
    }
}

fn apply_set_result(state: &mut AppState, id: StableId, result: Result<InputSource, String>) {
    match result {
        Ok(input) => {
            if let Some(m) = state.monitors.iter_mut().find(|m| m.id == id) {
                m.pending = false;
                m.current_input = Some(input);
            }
        }
        Err(e) => {
            if let Some(m) = state.monitors.iter_mut().find(|m| m.id == id) {
                m.pending = false;
            }
            state.status = e;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(model: &str) -> StableId {
        crate::ddc::identity::stable_id(Some("acme"), Some(model), None, Some("serial"), None, 0)
    }

    #[test]
    fn merge_updates_existing_and_preserves_pending_and_appends_new() {
        let mut existing = vec![MonitorState {
            id: id("a"),
            name: "A".into(),
            current_input: Some(InputSource(0x11)),
            valid_inputs: vec![InputSource(0x11)],
            available: true,
            owner: Owner::Me,
            pending: true,
        }];
        let incoming = vec![
            MonitorSnapshot {
                id: id("a"),
                name: "A".into(),
                current_input: Some(InputSource(0x12)),
                valid_inputs: vec![InputSource(0x11), InputSource(0x12)],
                available: true,
            },
            MonitorSnapshot {
                id: id("b"),
                name: "B".into(),
                current_input: None,
                valid_inputs: vec![],
                available: true,
            },
        ];

        merge_snapshots(&mut existing, incoming);

        assert_eq!(existing.len(), 2);
        assert_eq!(existing[0].current_input, Some(InputSource(0x12)));
        assert_eq!(existing[0].owner, Owner::Me, "owner must be preserved across a merge");
        assert!(existing[0].pending, "pending flag must be preserved across a merge");
        assert_eq!(existing[1].id, id("b"));
        assert!(!existing[1].pending);
    }

    #[test]
    fn merge_marks_monitors_missing_from_the_snapshot_unavailable() {
        let mut existing = vec![MonitorState {
            id: id("a"),
            name: "A".into(),
            current_input: Some(InputSource(0x11)),
            valid_inputs: vec![InputSource(0x11)],
            available: true,
            owner: Owner::Unknown,
            pending: false,
        }];

        merge_snapshots(&mut existing, vec![]);

        assert!(!existing[0].available);
    }

    #[test]
    fn set_result_ok_clears_pending_and_updates_input() {
        let mut state = AppState {
            monitors: vec![MonitorState {
                id: id("a"),
                name: "A".into(),
                current_input: Some(InputSource(0x11)),
                valid_inputs: vec![InputSource(0x11), InputSource(0x12)],
                available: true,
                owner: Owner::Me,
                pending: true,
            }],
            status: String::new(),
        };

        apply_set_result(&mut state, id("a"), Ok(InputSource(0x12)));

        assert!(!state.monitors[0].pending);
        assert_eq!(state.monitors[0].current_input, Some(InputSource(0x12)));
    }

    #[test]
    fn set_result_err_clears_pending_and_sets_status() {
        let mut state = AppState {
            monitors: vec![MonitorState {
                id: id("a"),
                name: "A".into(),
                current_input: Some(InputSource(0x11)),
                valid_inputs: vec![InputSource(0x11)],
                available: true,
                owner: Owner::Me,
                pending: true,
            }],
            status: String::new(),
        };

        apply_set_result(&mut state, id("a"), Err("write failed".to_string()));

        assert!(!state.monitors[0].pending);
        assert_eq!(state.status, "write failed");
    }
}
