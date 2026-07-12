//! Pure state-mutation helpers driven by worker/network events. Kept free of
//! egui so they can be unit tested without a UI context.

use std::sync::mpsc::Sender;

use crate::ddc::{InputSource, StableId};
use crate::model::{AppState, MonitorState};
use crate::net::NetEvt;
use crate::worker::{MonitorSnapshot, WorkerCmd};

pub(super) fn apply_net_event(state: &mut AppState, cmd_tx: &Sender<WorkerCmd>, evt: NetEvt) {
    match evt {
        NetEvt::PeerOnline { name } => {
            state.peer_online = true;
            state.peer_name = Some(name);
        }
        NetEvt::PeerOffline => state.peer_online = false,
        NetEvt::Hosts(hosts) => state.hosts = hosts,
        NetEvt::ExecuteSwitch { id, target } => {
            cmd_tx
                .send(WorkerCmd::SetInput { id, input: target })
                .ok();
        }
        NetEvt::Status(s) => state.status = s,
    }
}

/// Update `existing` in place from a fresh worker snapshot: matched monitors
/// keep their `pending` flag, unmatched incoming ones are appended, and existing
/// ones missing from `incoming` are marked unavailable. Never rebuilds the vec
/// wholesale so egui's widget state stays stable.
pub(super) fn merge_snapshots(existing: &mut Vec<MonitorState>, incoming: Vec<MonitorSnapshot>) {
    for snap in &incoming {
        if let Some(m) = existing.iter_mut().find(|m| m.id == snap.id) {
            m.name = snap.name.clone();
            m.controllable = snap.controllable;
            m.current_input = snap.current_input;
            m.observed_inputs = snap.observed_inputs.clone();
            m.available = snap.available;
        } else {
            existing.push(MonitorState {
                id: snap.id.clone(),
                name: snap.name.clone(),
                controllable: snap.controllable,
                current_input: snap.current_input,
                observed_inputs: snap.observed_inputs.clone(),
                available: snap.available,
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

pub(super) fn apply_set_result(state: &mut AppState, id: StableId, result: Result<InputSource, String>) {
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

    fn monitor(current: u8, observed: &[u8]) -> MonitorState {
        MonitorState {
            id: id("a"),
            name: "A".into(),
            controllable: true,
            current_input: Some(InputSource(current)),
            observed_inputs: observed.iter().copied().map(InputSource).collect(),
            available: true,
            pending: true,
        }
    }

    #[test]
    fn merge_updates_existing_and_preserves_pending_and_appends_new() {
        let mut existing = vec![monitor(0x11, &[0x11])];
        let incoming = vec![
            MonitorSnapshot {
                id: id("a"),
                name: "A".into(),
                controllable: true,
                current_input: Some(InputSource(0x12)),
                observed_inputs: vec![InputSource(0x11), InputSource(0x12)],
                available: true,
            },
            MonitorSnapshot {
                id: id("b"),
                name: "B".into(),
                controllable: false,
                current_input: None,
                observed_inputs: vec![],
                available: true,
            },
        ];

        merge_snapshots(&mut existing, incoming);

        assert_eq!(existing.len(), 2);
        assert_eq!(existing[0].current_input, Some(InputSource(0x12)));
        assert_eq!(existing[0].observed_inputs, vec![InputSource(0x11), InputSource(0x12)]);
        assert!(existing[0].pending, "pending flag must be preserved across a merge");
        assert_eq!(existing[1].id, id("b"));
        assert!(!existing[1].controllable);
        assert!(!existing[1].pending);
    }

    #[test]
    fn merge_marks_monitors_missing_from_the_snapshot_unavailable() {
        let mut existing = vec![monitor(0x11, &[0x11])];
        merge_snapshots(&mut existing, vec![]);
        assert!(!existing[0].available);
    }

    #[test]
    fn set_result_ok_clears_pending_and_updates_input() {
        let mut state = AppState {
            monitors: vec![monitor(0x11, &[0x11, 0x12])],
            ..Default::default()
        };

        apply_set_result(&mut state, id("a"), Ok(InputSource(0x12)));

        assert!(!state.monitors[0].pending);
        assert_eq!(state.monitors[0].current_input, Some(InputSource(0x12)));
    }

    #[test]
    fn set_result_err_clears_pending_and_sets_status() {
        let mut state = AppState {
            monitors: vec![monitor(0x11, &[0x11])],
            ..Default::default()
        };

        apply_set_result(&mut state, id("a"), Err("write failed".to_string()));

        assert!(!state.monitors[0].pending);
        assert_eq!(state.status, "write failed");
    }
}
