use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ddc::{InputSource, StableId};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct MonitorInfo {
    pub id: StableId,
    pub controllable: bool,
    pub current_input: Option<InputSource>,
    pub observed_inputs: Vec<InputSource>,
    pub label: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum Message {
    Hello {
        machine_id: Uuid,
        machine_name: String,
        monitors: Vec<MonitorInfo>,
    },
    State { monitors: Vec<MonitorInfo> },
    SwitchRequest { id: StableId, target: InputSource },
    /// The sender dropped the link on purpose and should not be redialed.
    Unlink,
    Heartbeat,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(model: &str) -> StableId {
        crate::ddc::identity::stable_id(Some("acme"), Some(model), None, Some("serial"), None)
    }

    fn sample_monitors() -> Vec<MonitorInfo> {
        vec![MonitorInfo {
            id: id("a"),
            controllable: true,
            current_input: Some(InputSource(0x12)),
            observed_inputs: vec![InputSource(0x11), InputSource(0x12)],
            label: "Monitor A".into(),
        }]
    }

    fn round_trip(msg: &Message) {
        let json = serde_json::to_string(msg).unwrap();
        assert!(!json.contains('\n'), "framing relies on newline-free JSON");
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(*msg, back);
    }

    #[test]
    fn all_variants_round_trip() {
        round_trip(&Message::Hello {
            machine_id: Uuid::new_v4(),
            machine_name: "pc".into(),
            monitors: sample_monitors(),
        });
        round_trip(&Message::State {
            monitors: sample_monitors(),
        });
        round_trip(&Message::SwitchRequest {
            id: id("a"),
            target: InputSource(0x11),
        });
        round_trip(&Message::Unlink);
        round_trip(&Message::Heartbeat);
    }
}
