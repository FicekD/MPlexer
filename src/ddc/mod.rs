pub mod edid;
pub mod identity;
pub mod inputs;

pub use edid::EdidInfo;
pub use identity::StableId;
pub use inputs::InputSource;

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use ddc_hi::{Ddc, Display, DisplayInfo};

const VCP_INPUT_SOURCE: u8 = 0x60;

const READ_RETRIES: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(50);
// Consecutive failed reads after which the last known input is no longer trusted.
// Many panels stop answering DDC on a cable once they show another input.
const STALE_AFTER_MISSES: u8 = 2;

pub struct Monitor {
    pub id: StableId,
    pub name: String,
    pub controllable: bool,
    pub current_input: Option<InputSource>,
    pub caps_inputs: Vec<InputSource>,
    pub edid: EdidInfo,
    handle: Display,
    misses: u8,
}

struct Candidate {
    display: Display,
    has_edid: bool,
    model_key: Option<String>,
    current_input: Option<InputSource>,
    caps_inputs: Vec<InputSource>,
}

pub fn enumerate() -> Vec<Monitor> {
    log::info!("scanning for monitors\u{2026}");
    let raw = Display::enumerate();
    log::info!("ddc-hi enumerate() returned {} raw display handle(s)", raw.len());

    let mut candidates: Vec<Candidate> = raw
        .into_iter()
        .enumerate()
        .map(|(index, mut display)| {
            let backend = display.info.backend;
            log::debug!("[{index}] probing via {backend} (id={})", display.info.id);

            // Read once and parse locally; `update_capabilities` would fetch the
            // whole string over DDC a second time.
            let caps_string = match display.handle.capabilities_string() {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    log::warn!("[{index}] {backend}: failed to read capabilities string: {e:#}");
                    String::new()
                }
            };
            if !caps_string.is_empty() {
                match mccs_caps::parse_capabilities(&caps_string) {
                    Ok(caps) => {
                        let from_caps =
                            DisplayInfo::from_capabilities(backend, display.info.id.clone(), &caps);
                        display.info.update_from(&from_caps);
                    }
                    Err(e) => {
                        log::debug!("[{index}] {backend}: capabilities string did not parse: {e:#}")
                    }
                }
            }

            let info = &display.info;
            let has_edid = info.edid_data.is_some();
            let edid_len = info.edid_data.as_ref().map_or(0, Vec::len);
            log::debug!(
                "[{index}] {backend}: model={:?} mfr={:?} serial={:?}/{:?} edid={}",
                info.model_name,
                info.manufacturer_id,
                info.serial_number,
                info.serial,
                if has_edid { format!("{edid_len} bytes") } else { "none".into() },
            );

            let log_name = info
                .model_name
                .clone()
                .unwrap_or_else(|| format!("Monitor {index}"));
            let model_key = info
                .model_name
                .as_deref()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty());

            let retries = if has_edid || model_key.is_some() { READ_RETRIES } else { 1 };
            let current_input = read_current_input(&mut display, &log_name, retries);
            match current_input {
                Some(i) => log::debug!("[{index}] {log_name}: current input {}", i.label()),
                None => log::debug!("[{index}] {log_name}: input source unreadable over DDC/CI"),
            }

            let caps_inputs = inputs::parse_input_list(&caps_string);
            log::debug!(
                "[{index}] {log_name}: capabilities string {} bytes, advertises {} input(s): [{}]",
                caps_string.len(),
                caps_inputs.len(),
                labels(&caps_inputs),
            );

            Candidate {
                display,
                has_edid,
                model_key,
                current_input,
                caps_inputs,
            }
        })
        .collect();

    log::debug!("{} candidate(s) before dedup/ghost filtering", candidates.len());
    drop_backend_duplicates(&mut candidates);

    candidates.retain(|c| {
        let communicating = c.current_input.is_some() || !c.caps_inputs.is_empty();
        let keep = !is_ghost(c.has_edid, c.model_key.is_some(), communicating);
        if !keep {
            log::info!(
                "dropping ghost handle ({}): no EDID, no model name, no DDC response",
                c.display.info.backend,
            );
        }
        keep
    });

    let mut ids: Vec<StableId> = candidates
        .iter()
        .map(|c| {
            let info = &c.display.info;
            identity::stable_id(
                info.manufacturer_id.as_deref(),
                info.model_name.as_deref(),
                info.model_id,
                info.serial_number.as_deref(),
                info.serial,
            )
        })
        .collect();
    identity::disambiguate(&mut ids);

    let monitors: Vec<Monitor> = candidates
        .into_iter()
        .zip(ids)
        .enumerate()
        .map(|(index, (c, id))| {
            let info = &c.display.info;
            let name = info
                .model_name
                .clone()
                .unwrap_or_else(|| format!("Monitor {index}"));

            let controllable = c.current_input.is_some() || !c.caps_inputs.is_empty();
            let edid = edid::extract(
                info.edid_data.as_deref(),
                info.serial_number.as_deref(),
                info.serial,
            );

            log::info!(
                "detected monitor [{index}] '{name}' (id={id}) controllable={controllable} \
                 current={} serial={} inputs=[{}]",
                c.current_input.map_or_else(|| "?".into(), |i| i.label()),
                edid.serial.as_deref().unwrap_or("?"),
                labels(&c.caps_inputs),
            );

            Monitor {
                id,
                name,
                controllable,
                current_input: c.current_input,
                caps_inputs: c.caps_inputs,
                edid,
                handle: c.display,
                misses: 0,
            }
        })
        .collect();

    log::info!("discovery complete: {} monitor(s) detected", monitors.len());
    monitors
}

fn labels(inputs: &[InputSource]) -> String {
    inputs.iter().map(|i| i.label()).collect::<Vec<_>>().join(" ")
}

fn read_current_input(display: &mut Display, log_name: &str, retries: usize) -> Option<InputSource> {
    for attempt in 1..=retries {
        match display.handle.get_vcp_feature(VCP_INPUT_SOURCE) {
            Ok(v) => {
                if attempt > 1 {
                    log::debug!("VCP 0x60 on {log_name} succeeded on attempt {attempt}/{retries}");
                }
                return Some(InputSource(v.value() as u8));
            }
            Err(e) if attempt == retries => {
                log::warn!("failed to read VCP 0x60 on {log_name} after {attempt} attempt(s): {e:#}");
            }
            Err(e) => {
                log::debug!(
                    "VCP 0x60 on {log_name} attempt {attempt}/{retries} failed: {e:#}; retrying in {}ms",
                    RETRY_DELAY.as_millis(),
                );
                std::thread::sleep(RETRY_DELAY);
            }
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
    candidates.retain(|c| {
        let dup = is_duplicate.next().unwrap();
        if dup {
            log::info!(
                "dropping backend-duplicate handle ({}) for '{}': EDID-less copy of an EDID-bearing panel",
                c.display.info.backend,
                c.model_key.as_deref().unwrap_or("?"),
            );
        }
        !dup
    });
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
        match self.handle.handle.get_vcp_feature(VCP_INPUT_SOURCE) {
            Ok(value) => {
                let input = InputSource(value.value() as u8);
                self.misses = 0;
                self.current_input = Some(input);
                Ok(input)
            }
            Err(e) => {
                self.misses = self.misses.saturating_add(1);
                if self.misses >= STALE_AFTER_MISSES {
                    self.current_input = None;
                }
                Err(e).with_context(|| format!("VCP 0x60 on {}", self.name))
            }
        }
    }

    pub fn write_input(&mut self, input: InputSource) -> Result<()> {
        self.handle
            .handle
            .set_vcp_feature(VCP_INPUT_SOURCE, input.0 as u16)
            .with_context(|| format!("VCP 0x60 on {}", self.name))?;
        self.misses = 0;
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
