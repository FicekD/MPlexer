use std::collections::HashMap;

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

/// Builds the id from what the EDID says about the panel, so every machine
/// cabled to the same monitor derives the same string. Panels that share a
/// result (no serial, or a zero placeholder) are split later by `disambiguate`.
pub fn stable_id(
    manufacturer: Option<&str>,
    model_name: Option<&str>,
    model_id: Option<u16>,
    serial_number: Option<&str>,
    serial: Option<u32>,
) -> StableId {
    let manufacturer = normalize(manufacturer);
    let model = normalize(model_name).or_else(|| model_id.map(|id| format!("model-{id:04x}")));

    if manufacturer.is_none() && model.is_none() {
        log::warn!("display has no manufacturer or model info");
        return StableId("unknown".to_string());
    }

    let mut parts = vec![
        manufacturer.unwrap_or_else(|| "unknown".to_string()),
        model.unwrap_or_else(|| "unknown".to_string()),
    ];
    if let Some(unique) =
        normalize(serial_number).or_else(|| serial.filter(|s| *s != 0).map(|s| s.to_string()))
    {
        parts.push(unique);
    }
    StableId(parts.join("-"))
}

/// Numbers ids that occur more than once, in enumeration order. A panel whose id
/// is already unique keeps it untouched, which is what lets two machines agree
/// on a serial-less monitor.
pub fn disambiguate(ids: &mut [StableId]) {
    let originals = ids.to_vec();
    let mut next: HashMap<StableId, usize> = HashMap::new();
    for id in ids.iter_mut() {
        if originals.iter().filter(|o| *o == id).count() < 2 {
            continue;
        }
        let n = next.entry(id.clone()).or_default();
        log::warn!("several displays share id {id}; numbering this one {n} by enumeration order");
        id.0 = format!("{}-idx{n}", id.0);
        *n += 1;
    }
}

fn normalize(s: Option<&str>) -> Option<String> {
    s.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_inputs_produce_equal_ids() {
        let a = stable_id(
            Some("DEL"),
            Some("U2419H"),
            Some(0xA1B2),
            Some("ABC123"),
            None,
        );
        let b = stable_id(
            Some("DEL"),
            Some("U2419H"),
            Some(0xA1B2),
            Some("ABC123"),
            None,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn differing_serials_produce_different_ids() {
        let a = stable_id(Some("DEL"), Some("U2419H"), None, Some("ABC123"), None);
        let b = stable_id(Some("DEL"), Some("U2419H"), None, Some("XYZ999"), None);
        assert_ne!(a, b);
    }

    #[test]
    fn numeric_serial_used_when_no_serial_number_string() {
        let a = stable_id(Some("DEL"), Some("U2419H"), None, None, Some(42));
        let b = stable_id(Some("DEL"), Some("U2419H"), None, None, Some(43));
        assert_ne!(a, b);
    }

    #[test]
    fn serial_less_identical_panels_are_kept_apart_by_order() {
        let mut ids = vec![
            stable_id(Some("DEL"), Some("U2419H"), None, None, None),
            stable_id(Some("ACME"), Some("X"), None, Some("1"), None),
            stable_id(Some("DEL"), Some("U2419H"), None, None, None),
        ];
        disambiguate(&mut ids);
        assert_eq!(ids[0].as_str(), "del-u2419h-idx0");
        assert_eq!(ids[1].as_str(), "acme-x-1");
        assert_eq!(ids[2].as_str(), "del-u2419h-idx1");
    }

    #[test]
    fn no_identifying_info_is_unknown() {
        assert_eq!(stable_id(None, None, None, None, None).as_str(), "unknown");
    }

    #[test]
    fn normalization_trims_and_lowercases() {
        let a = stable_id(
            Some(" Dell "),
            Some(" U2419H "),
            None,
            Some(" ABC123 "),
            None,
        );
        let b = stable_id(Some("dell"), Some("u2419h"), None, Some("abc123"), None);
        assert_eq!(a, b);
    }
}
