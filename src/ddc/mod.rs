pub mod identity;
pub mod inputs;

pub use identity::StableId;
pub use inputs::InputSource;

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use ddc_hi::{Ddc, Display};

const VCP_INPUT_SOURCE: u8 = 0x60;

const READ_RETRIES: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(50);

pub struct Monitor {
    pub id: StableId,
    pub name: String,
    pub controllable: bool,
    pub current_input: Option<InputSource>,
    pub caps_inputs: Vec<InputSource>,
    handle: Display,
}

struct Candidate {
    display: Display,
    has_edid: bool,
    model_key: Option<String>,
    current_input: Option<InputSource>,
    caps_inputs: Vec<InputSource>,
}

pub fn enumerate() -> Vec<Monitor> {
    let mut candidates: Vec<Candidate> = Display::enumerate()
        .into_iter()
        .enumerate()
        .map(|(index, mut display)| {
            if let Err(e) = display.update_capabilities() {
                log::warn!("failed to read capabilities for display {index}: {e:#}");
            }

            let log_name = display
                .info
                .model_name
                .clone()
                .unwrap_or_else(|| format!("Monitor {index}"));

            let has_edid = display.info.edid_data.is_some();
            let model_key = display
                .info
                .model_name
                .as_deref()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty());

            let retries = if has_edid || model_key.is_some() { READ_RETRIES } else { 1 };
            let current_input = read_current_input(&mut display, &log_name, retries);

            let caps_string = match display.handle.capabilities_string() {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    log::warn!("failed to read capabilities string on {log_name}: {e:#}");
                    String::new()
                }
            };
            let caps_inputs = inputs::parse_input_list(&caps_string);

            Candidate {
                display,
                has_edid,
                model_key,
                current_input,
                caps_inputs,
            }
        })
        .collect();

    drop_backend_duplicates(&mut candidates);

    candidates.retain(|c| {
        let communicating = c.current_input.is_some() || !c.caps_inputs.is_empty();
        let keep = !is_ghost(c.has_edid, c.model_key.is_some(), communicating);
        if !keep {
            log::debug!("dropping anonymous non-responsive display handle");
        }
        keep
    });

    candidates
        .into_iter()
        .enumerate()
        .map(|(index, c)| {
            let info = &c.display.info;
            let id = identity::stable_id(
                info.manufacturer_id.as_deref(),
                info.model_name.as_deref(),
                info.model_id,
                info.serial_number.as_deref(),
                info.serial,
                index,
            );
            let name = info
                .model_name
                .clone()
                .unwrap_or_else(|| format!("Monitor {index}"));

            let controllable = c.current_input.is_some() || !c.caps_inputs.is_empty();

            Monitor {
                id,
                name,
                controllable,
                current_input: c.current_input,
                caps_inputs: c.caps_inputs,
                handle: c.display,
            }
        })
        .collect()
}

fn read_current_input(display: &mut Display, log_name: &str, retries: usize) -> Option<InputSource> {
    for attempt in 1..=retries {
        match display.handle.get_vcp_feature(VCP_INPUT_SOURCE) {
            Ok(v) => return Some(InputSource(v.value() as u8)),
            Err(e) if attempt == retries => {
                log::warn!("failed to read VCP 0x60 on {log_name} after {attempt} attempt(s): {e:#}");
            }
            Err(_) => std::thread::sleep(RETRY_DELAY),
        }
    }
    None
}

fn drop_backend_duplicates(candidates: &mut Vec<Candidate>) {
    let entries: Vec<(bool, Option<&str>)> = candidates
        .iter()
        .map(|c| (c.has_edid, c.model_key.as_deref()))
        .collect();
    let mut is_duplicate = duplicate_flags(&entries).into_iter();
    candidates.retain(|_| !is_duplicate.next().unwrap());
}

fn is_ghost(has_edid: bool, has_model: bool, communicating: bool) -> bool {
    !has_edid && !has_model && !communicating
}

fn duplicate_flags(entries: &[(bool, Option<&str>)]) -> Vec<bool> {
    let rich_models: HashSet<&str> = entries
        .iter()
        .filter(|(has_edid, _)| *has_edid)
        .filter_map(|(_, model)| *model)
        .collect();
    entries
        .iter()
        .map(|(has_edid, model)| !*has_edid && model.is_some_and(|m| rich_models.contains(m)))
        .collect()
}

impl Monitor {
    pub fn read_input(&mut self) -> Result<InputSource> {
        let value = self
            .handle
            .handle
            .get_vcp_feature(VCP_INPUT_SOURCE)
            .with_context(|| format!("VCP 0x60 on {}", self.name))?;
        let input = InputSource(value.value() as u8);
        self.current_input = Some(input);
        Ok(input)
    }

    pub fn write_input(&mut self, input: InputSource) -> Result<()> {
        self.handle
            .handle
            .set_vcp_feature(VCP_INPUT_SOURCE, input.0 as u16)
            .with_context(|| format!("VCP 0x60 on {}", self.name))?;
        self.current_input = Some(input);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edidless_copy_of_edid_panel_is_dropped() {
        let entries = [(true, Some("msi md272qxp")), (false, Some("msi md272qxp"))];
        assert_eq!(duplicate_flags(&entries), vec![false, true]);
    }

    #[test]
    fn edidless_panel_with_no_rich_counterpart_is_kept() {
        let entries = [(false, Some("dell u2419h")), (false, Some("acme x"))];
        assert_eq!(duplicate_flags(&entries), vec![false, false]);
    }

    #[test]
    fn distinct_edid_panels_of_same_model_are_all_kept() {
        let entries = [(true, Some("msi md272qxp")), (true, Some("msi md272qxp"))];
        assert_eq!(duplicate_flags(&entries), vec![false, false]);
    }

    #[test]
    fn entry_without_model_name_is_never_treated_as_duplicate() {
        let entries = [(true, Some("msi md272qxp")), (false, None)];
        assert_eq!(duplicate_flags(&entries), vec![false, false]);
    }

    #[test]
    fn anonymous_nonresponsive_handle_is_a_ghost() {
        assert!(is_ghost(false, false, false));
    }

    #[test]
    fn edid_model_or_ddc_response_keeps_a_handle() {
        assert!(!is_ghost(true, false, false), "has EDID");
        assert!(!is_ghost(false, true, false), "has model name");
        assert!(!is_ghost(false, false, true), "responds over DDC");
    }
}
