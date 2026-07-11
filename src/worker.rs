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

    let mut last = snapshot(&monitors);
    if emit(&evt_tx, &ctx, WorkerEvt::Monitors(last.clone())).is_err() {
        return;
    }

    loop {
        match cmd_rx.recv_timeout(POLL_INTERVAL) {
            Ok(WorkerCmd::Refresh) => {
                monitors = ddc::enumerate();
                last = snapshot(&monitors);
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
                }
                if emit(&evt_tx, &ctx, WorkerEvt::SetResult { id, result }).is_err() {
                    return;
                }
                last = snapshot(&monitors);
                if emit(&evt_tx, &ctx, WorkerEvt::Monitors(last.clone())).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                for m in monitors.iter_mut() {
                    let _ = m.read_input();
                }
                let current = snapshot(&monitors);
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

fn snapshot(monitors: &[Monitor]) -> Vec<MonitorSnapshot> {
    monitors
        .iter()
        .map(|m| MonitorSnapshot {
            id: m.id.clone(),
            name: m.name.clone(),
            current_input: m.current_input,
            valid_inputs: m.valid_inputs.clone(),
            available: true,
        })
        .collect()
}

fn emit(evt_tx: &Sender<WorkerEvt>, ctx: &egui::Context, evt: WorkerEvt) -> Result<(), ()> {
    evt_tx.send(evt).map_err(|_| ())?;
    ctx.request_repaint();
    Ok(())
}
