use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::config::Config;
use crate::ddc::{self, InputSource, Monitor, StableId};

const POLL_INTERVAL: Duration = Duration::from_secs(3);
const SETTLE_DELAY: Duration = Duration::from_secs(1);

pub enum WorkerCmd {
    Refresh,
    SetInput { id: StableId, input: InputSource },
}

pub enum WorkerEvt {
    Monitors(Vec<MonitorSnapshot>),
    SetResult {
        id: StableId,
        result: Result<InputSource, String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonitorSnapshot {
    pub id: StableId,
    pub name: String,
    pub controllable: bool,
    pub current_input: Option<InputSource>,
    pub observed_inputs: Vec<InputSource>,
    pub available: bool,
}

pub fn spawn(
    cmd_rx: Receiver<WorkerCmd>,
    evt_tx: Sender<WorkerEvt>,
    ctx: egui::Context,
    observed: HashMap<StableId, Vec<InputSource>>,
) {
    std::thread::spawn(move || run(cmd_rx, evt_tx, ctx, observed));
}

fn run(
    cmd_rx: Receiver<WorkerCmd>,
    evt_tx: Sender<WorkerEvt>,
    ctx: egui::Context,
    mut observed: HashMap<StableId, Vec<InputSource>>,
) {
    let mut monitors = ddc::enumerate();
    log::info!("worker started: {} monitor(s) enumerated", monitors.len());
    seed_observed(&mut observed, &monitors);
    persist(&observed);

    let mut last = snapshot(&monitors, &observed);
    if emit(&evt_tx, &ctx, WorkerEvt::Monitors(last.clone())).is_err() {
        return;
    }

    loop {
        match cmd_rx.recv_timeout(POLL_INTERVAL) {
            Ok(WorkerCmd::Refresh) => {
                monitors = ddc::enumerate();
                log::info!("refresh: {} monitor(s) enumerated", monitors.len());
                if seed_observed(&mut observed, &monitors) {
                    persist(&observed);
                }
                last = snapshot(&monitors, &observed);
                if emit(&evt_tx, &ctx, WorkerEvt::Monitors(last.clone())).is_err() {
                    return;
                }
            }
            Ok(WorkerCmd::SetInput { id, input }) => {
                let mut result = Err(format!("monitor {id} not found"));
                if let Some(m) = monitors.iter_mut().find(|m| m.id == id) {
                    log::info!("switching '{}' to {}", m.name, input.label());
                    result = match m.write_input(input) {
                        Ok(()) => {
                            std::thread::sleep(SETTLE_DELAY);
                            m.read_input().map_err(|e| e.to_string())
                        }
                        Err(e) => Err(e.to_string()),
                    };
                }
                match &result {
                    Ok(got) => log::info!("switch of {id} confirmed at 0x{:02X}", got.0),
                    Err(e) => log::warn!("switch of {id} failed: {e}"),
                }
                if let Ok(got) = &result
                    && note_input(&mut observed, &id, *got)
                {
                    persist(&observed);
                }
                if emit(&evt_tx, &ctx, WorkerEvt::SetResult { id, result }).is_err() {
                    return;
                }
                last = snapshot(&monitors, &observed);
                if emit(&evt_tx, &ctx, WorkerEvt::Monitors(last.clone())).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let mut grew = false;
                for m in monitors.iter_mut() {
                    if let Ok(input) = m.read_input() {
                        grew |= note_input(&mut observed, &m.id, input);
                    }
                }
                if grew {
                    persist(&observed);
                }
                let current = snapshot(&monitors, &observed);
                if current != last {
                    last = current.clone();
                    if emit(&evt_tx, &ctx, WorkerEvt::Monitors(current)).is_err() {
                        return;
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn note_input(
    observed: &mut HashMap<StableId, Vec<InputSource>>,
    id: &StableId,
    input: InputSource,
) -> bool {
    let seen = observed.entry(id.clone()).or_default();
    if seen.contains(&input) {
        false
    } else {
        seen.push(input);
        true
    }
}

fn seed_observed(observed: &mut HashMap<StableId, Vec<InputSource>>, monitors: &[Monitor]) -> bool {
    let mut grew = false;
    for m in monitors {
        for &inp in &m.caps_inputs {
            grew |= note_input(observed, &m.id, inp);
        }
        if let Some(cur) = m.current_input {
            grew |= note_input(observed, &m.id, cur);
        }
    }
    grew
}

fn snapshot(
    monitors: &[Monitor],
    observed: &HashMap<StableId, Vec<InputSource>>,
) -> Vec<MonitorSnapshot> {
    monitors
        .iter()
        .map(|m| {
            let mut inputs: Vec<InputSource> = observed.get(&m.id).cloned().unwrap_or_default();
            // Show the active input first.
            if let Some(cur) = m.current_input
                && let Some(pos) = inputs.iter().position(|&i| i == cur)
            {
                inputs.remove(pos);
                inputs.insert(0, cur);
            }
            MonitorSnapshot {
                id: m.id.clone(),
                name: m.name.clone(),
                controllable: m.controllable || !inputs.is_empty(),
                current_input: m.current_input,
                observed_inputs: inputs,
                available: true,
            }
        })
        .collect()
}

fn persist(observed: &HashMap<StableId, Vec<InputSource>>) {
    let snapshot = observed.clone();
    if let Err(e) = Config::persist_with(move |c| c.observed_inputs = snapshot) {
        log::warn!("failed to persist observed inputs: {e:#}");
    }
}

fn emit(evt_tx: &Sender<WorkerEvt>, ctx: &egui::Context, evt: WorkerEvt) -> Result<(), ()> {
    evt_tx.send(evt).map_err(|_| ())?;
    ctx.request_repaint();
    Ok(())
}
