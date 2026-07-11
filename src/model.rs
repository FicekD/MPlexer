use crate::ddc::{InputSource, StableId};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    Me,
    #[allow(dead_code)]
    Peer,
    #[allow(dead_code)]
    Unknown,
}

#[derive(Clone, Debug)]
pub struct MonitorState {
    pub id: StableId,
    pub name: String,
    pub current_input: Option<InputSource>,
    pub valid_inputs: Vec<InputSource>,
    pub available: bool,
    pub owner: Owner,
    pub pending: bool,
}

#[derive(Default)]
pub struct AppState {
    pub monitors: Vec<MonitorState>,
    pub status: String,
}
