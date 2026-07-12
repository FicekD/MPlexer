use crate::ddc::{InputSource, StableId};
use crate::net::HostView;

#[derive(Clone, Debug)]
pub struct MonitorState {
    pub id: StableId,
    pub name: String,
    pub controllable: bool,
    pub current_input: Option<InputSource>,
    pub observed_inputs: Vec<InputSource>,
    pub available: bool,
    pub pending: bool,
}

#[derive(Default)]
pub struct AppState {
    pub monitors: Vec<MonitorState>,
    pub status: String,
    pub peer_online: bool,
    pub peer_name: Option<String>,
    pub hosts: Vec<HostView>,
}
