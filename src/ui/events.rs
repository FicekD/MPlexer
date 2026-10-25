//! Pure state-mutation helpers driven by worker/network events. Kept free of
//! egui so they can be unit tested without a UI context.

use std::sync::mpsc::Sender;

use crate::ddc::{InputSource, StableId};
use crate::model::{AppState, MonitorState};
use crate::net::NetEvt;
use crate::net::proto::MonitorInfo;
use crate::worker::{MonitorSnapshot, WorkerCmd};

pub(super) fn apply_net_event(state: &mut AppState, cmd_tx: &Sender<WorkerCmd>, evt: NetEvt) {
    match evt {
        NetEvt::PeerOnline { name } => {
            state.peer_online = true;
            state.peer_name = Some(name);
        }
        NetEvt::PeerOffline => state.peer_online = false,
        NetEvt::Hosts(hosts) => state.hosts = hosts,
        NetEvt::PeerMonitors(infos) => {
            let learned: Vec<_> = infos
                .iter()
                .filter(|i| !i.observed_inputs.is_empty())
                .map(|i| (i.id.clone(), i.observed_inputs.clone()))
                .collect();
            if !learned.is_empty() {
                cmd_tx.send(WorkerCmd::LearnInputs(learned)).ok();
            }
            apply_peer_monitors(&mut state.monitors, infos);
        }
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
            m.local = true;
            m.name = snap.name.clone();
            m.controllable = snap.controllable;
            m.current_input = snap.current_input;
            m.observed_inputs = snap.observed_inputs.clone();
            m.available = snap.available;
            m.edid = snap.edid.clone();
        } else {
            existing.push(MonitorState {
                id: snap.id.clone(),
                name: snap.name.clone(),
                controllable: snap.controllable,
                current_input: snap.current_input,
                observed_inputs: snap.observed_inputs.clone(),
                available: snap.available,
                pending: false,
                edid: snap.edid.clone(),
                local: true,
                peer: None,
            });
        }
    }
    for m in existing.iter_mut().filter(|m| m.local) {
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
            let mut handed_off = false;
            if let Some(m) = state.monitors.iter_mut().find(|m| m.id == id) {
                m.pending = false;
                handed_off = m.switchable_by_peer();
            }
            state.status = if handed_off {
                format!(
                    "switch handed to {}",
                    state.peer_name.as_deref().unwrap_or("peer")
                )
            } else {
                e
            };
        }
    }
}

/// Attach the peer's view to matching tiles, add tiles for monitors only the
/// peer is cabled to, and drop those once the peer stops reporting them.
/// Switches sent only to the peer settle with its next report.
pub(super) fn apply_peer_monitors(monitors: &mut Vec<MonitorState>, infos: Vec<MonitorInfo>) {
    for m in monitors.iter_mut() {
        m.peer = infos.iter().find(|i| i.id == m.id).cloned();
        if !m.switchable_here() {
            m.pending = false;
        }
    }
    monitors.retain(|m| m.local || m.peer.is_some());
    for info in infos {
        if !monitors.iter().any(|m| m.id == info.id) {
            monitors.push(MonitorState {
                id: info.id.clone(),
                name: info.label.clone(),
                controllable: false,
                current_input: None,
                observed_inputs: Vec::new(),
                available: false,
                pending: false,
                edid: Default::default(),
                local: false,
                peer: Some(info),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(model: &str) -> StableId {
        crate::ddc::identity::stable_id(Some("acme"), Some(model), None, Some("serial"), None)
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
            edid: Default::default(),
            local: true,
            peer: None,
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
                edid: Default::default(),
            },
            MonitorSnapshot {
                id: id("b"),
                name: "B".into(),
                controllable: false,
                current_input: None,
                observed_inputs: vec![],
                available: true,
                edid: Default::default(),
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
