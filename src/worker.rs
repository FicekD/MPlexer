use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

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
    pub current_input: Option<InputSource>,
    pub valid_inputs: Vec<InputSource>,
    pub available: bool,
}

pub fn spawn(cmd_rx: Receiver<WorkerCmd>, evt_tx: Sender<WorkerEvt>, ctx: egui::Context) {
    std::thread::spawn(move || run(cmd_rx, evt_tx, ctx));
}

fn run(cmd_rx: Receiver<WorkerCmd>, evt_tx: Sender<WorkerEvt>, ctx: egui::Context) {
    let mut monitors = ddc::enumerate();

    let mut available: Vec<bool> = monitors.iter().map(|m| m.current_input.is_some()).collect();
    let mut last = snapshot(&monitors, &available);
    if emit(&evt_tx, &ctx, WorkerEvt::Monitors(last.clone())).is_err() {
        return;
    }

    loop {
        match cmd_rx.recv_timeout(POLL_INTERVAL) {
            Ok(WorkerCmd::Refresh) => {
                monitors = ddc::enumerate();
                available = monitors.iter().map(|m| m.current_input.is_some()).collect();
                last = snapshot(&monitors, &available);
                if emit(&evt_tx, &ctx, WorkerEvt::Monitors(last.clone())).is_err() {
                    return;
                }
            }
            Ok(WorkerCmd::SetInput { id, input }) => {
                let mut result = Err(format!("monitor {id} not found"));
                if let Some(idx) = monitors.iter().position(|m| m.id == id) {
                    let m = &mut monitors[idx];
                    result = match m.write_input(input) {
                        Ok(()) => {
                            std::thread::sleep(SETTLE_DELAY);
                            m.read_input().map_err(|e| e.to_string())
                        }
                        Err(e) => Err(e.to_string()),
                    };
                    available[idx] = result.is_ok();
                }
                if emit(&evt_tx, &ctx, WorkerEvt::SetResult { id, result }).is_err() {
                    return;
                }
                last = snapshot(&monitors, &available);
                if emit(&evt_tx, &ctx, WorkerEvt::Monitors(last.clone())).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                for (m, avail) in monitors.iter_mut().zip(available.iter_mut()) {
                    *avail = m.read_input().is_ok();
                }
                let current = snapshot(&monitors, &available);
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

fn snapshot(monitors: &[Monitor], available: &[bool]) -> Vec<MonitorSnapshot> {
    monitors
        .iter()
        .zip(available.iter())
        .map(|(m, &available)| MonitorSnapshot {
            id: m.id.clone(),
            name: m.name.clone(),
            current_input: m.current_input,
            valid_inputs: m.valid_inputs.clone(),
            available,
        })
        .collect()
}

fn emit(evt_tx: &Sender<WorkerEvt>, ctx: &egui::Context, evt: WorkerEvt) -> Result<(), ()> {
    evt_tx.send(evt).map_err(|_| ())?;
    ctx.request_repaint();
    Ok(())
}
