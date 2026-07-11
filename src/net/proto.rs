use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ddc::{InputSource, StableId};

/// One machine's view of a single monitor, shared over the wire.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct MonitorInfo {
    pub id: StableId,
    /// The sender's own input on this monitor (its "home" input).
    pub my_input: Option<InputSource>,
    /// What the sender currently reads; `None` when the monitor is unreachable from its side.
    pub current_input: Option<InputSource>,
    pub valid_inputs: Vec<InputSource>,
    pub label: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum Message {
    /// First message after TCP connect: identity plus a full monitor view.
    Hello {
        machine_id: Uuid,
        machine_name: String,
        monitors: Vec<MonitorInfo>,
    },
    /// Full state push, authoritative from the sender's side.
    State { monitors: Vec<MonitorInfo> },
    /// "Please switch monitor <id> to <target>." Executed by the current owner.
    SwitchRequest { id: StableId, target: InputSource },
    /// Result of a SwitchRequest the receiver executed.
    SwitchResult {
        id: StableId,
        ok: bool,
        current: Option<InputSource>,
        error: Option<String>,
    },
    Heartbeat,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(model: &str) -> StableId {
        crate::ddc::identity::stable_id(Some("acme"), Some(model), None, Some("serial"), None, 0)
    }

    fn sample_monitors() -> Vec<MonitorInfo> {
        vec![MonitorInfo {
            id: id("a"),
            my_input: Some(InputSource(0x11)),
            current_input: Some(InputSource(0x12)),
            valid_inputs: vec![InputSource(0x11), InputSource(0x12)],
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
        round_trip(&Message::SwitchResult {
            id: id("a"),
            ok: true,
            current: Some(InputSource(0x11)),
            error: None,
        });
        round_trip(&Message::SwitchResult {
            id: id("a"),
            ok: false,
            current: None,
            error: Some("not owner".into()),
        });
        round_trip(&Message::Heartbeat);
    }
}
