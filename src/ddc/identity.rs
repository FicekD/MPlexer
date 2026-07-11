#[derive(Clone, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct StableId(String);

impl StableId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for StableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn stable_id(
    manufacturer: Option<&str>,
    model_name: Option<&str>,
    model_id: Option<u16>,
    serial_number: Option<&str>,
    serial: Option<u32>,
    fallback_index: usize,
) -> StableId {
    let manufacturer = normalize(manufacturer);
    let model = normalize(model_name).or_else(|| model_id.map(|id| format!("model-{id:04x}")));

    if manufacturer.is_none() && model.is_none() {
        log::warn!(
            "display has no manufacturer or model info, falling back to enumeration index {fallback_index}"
        );
        return StableId(format!("unknown-{fallback_index}"));
    }

    let unique = normalize(serial_number).or_else(|| serial.map(|s| s.to_string()));
    if unique.is_none() {
        log::warn!(
            "display {}/{} has no serial number, its StableId may collide with an identical panel",
            manufacturer.as_deref().unwrap_or("unknown"),
            model.as_deref().unwrap_or("unknown"),
        );
    }

    let parts = [
        manufacturer.unwrap_or_else(|| "unknown".to_string()),
        model.unwrap_or_else(|| "unknown".to_string()),
        unique.unwrap_or_default(),
    ];
    StableId(parts.join("-"))
}

fn normalize(s: Option<&str>) -> Option<String> {
    s.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_inputs_produce_equal_ids() {
        let a = stable_id(Some("DEL"), Some("U2419H"), Some(0xA1B2), Some("ABC123"), None, 0);
        let b = stable_id(Some("DEL"), Some("U2419H"), Some(0xA1B2), Some("ABC123"), None, 1);
        assert_eq!(a, b);
    }

    #[test]
    fn differing_serials_produce_different_ids() {
        let a = stable_id(Some("DEL"), Some("U2419H"), None, Some("ABC123"), None, 0);
        let b = stable_id(Some("DEL"), Some("U2419H"), None, Some("XYZ999"), None, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn numeric_serial_used_when_no_serial_number_string() {
        let a = stable_id(Some("DEL"), Some("U2419H"), None, None, Some(42), 0);
        let b = stable_id(Some("DEL"), Some("U2419H"), None, None, Some(43), 0);
        assert_ne!(a, b);
    }

    #[test]
    fn missing_serial_falls_back_to_model_fields_deterministically() {
        let a = stable_id(Some("DEL"), Some("U2419H"), None, None, None, 0);
        let b = stable_id(Some("DEL"), Some("U2419H"), None, None, None, 7);
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "del-u2419h-");
    }

    #[test]
    fn no_identifying_info_falls_back_to_index() {
        let a = stable_id(None, None, None, None, None, 3);
        assert_eq!(a.as_str(), "unknown-3");
    }

    #[test]
    fn normalization_trims_and_lowercases() {
        let a = stable_id(Some(" Dell "), Some(" U2419H "), None, Some(" ABC123 "), None, 0);
        let b = stable_id(Some("dell"), Some("u2419h"), None, Some("abc123"), None, 0);
        assert_eq!(a, b);
    }
}
