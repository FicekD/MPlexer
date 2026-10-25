use crate::ddc::{EdidInfo, InputSource, StableId};
use crate::net::HostView;
use crate::net::proto::MonitorInfo;

#[derive(Clone, Debug)]
pub struct MonitorState {
    pub id: StableId,
    pub name: String,
    pub controllable: bool,
    pub current_input: Option<InputSource>,
    pub observed_inputs: Vec<InputSource>,
    pub available: bool,
    pub pending: bool,
    pub edid: EdidInfo,
    /// Known to this machine's worker; `false` for tiles that exist only
    /// because the linked peer is cabled to the monitor.
    pub local: bool,
    pub peer: Option<MonitorInfo>,
}

impl MonitorState {
    pub fn switchable_here(&self) -> bool {
        self.available && self.controllable
    }

    pub fn switchable_by_peer(&self) -> bool {
        self.peer.as_ref().is_some_and(|p| p.controllable)
    }

    pub fn reachable(&self) -> bool {
        self.available || self.peer.is_some()
    }

    /// The local reading wins; the peer's fills in when this machine can no
    /// longer read the panel, typically because it shows the peer's input.
    pub fn shown_input(&self) -> Option<InputSource> {
        self.current_input
            .filter(|_| self.available)
            .or_else(|| self.peer.as_ref()?.current_input)
    }

    pub fn inputs(&self) -> Vec<InputSource> {
        let mut inputs = self.observed_inputs.clone();
        for &input in self.peer.iter().flat_map(|p| &p.observed_inputs) {
            if !inputs.contains(&input) {
                inputs.push(input);
            }
        }
        inputs
    }
}

#[derive(Default)]
pub struct AppState {
    pub monitors: Vec<MonitorState>,
    pub status: String,
    pub peer_online: bool,
    pub peer_name: Option<String>,
    pub hosts: Vec<HostView>,
}
