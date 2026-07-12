use edid::Descriptor;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdidInfo {
    pub serial: Option<String>,
    pub manufacturer: Option<String>,
    pub product_code: Option<u16>,
    pub week: Option<u8>,
    pub year: Option<u16>,
    pub edid_version: Option<(u8, u8)>,
    pub size_cm: Option<(u8, u8)>,
    pub native: Option<(u16, u16)>,
}

impl EdidInfo {
    pub fn is_empty(&self) -> bool {
        *self == EdidInfo::default()
    }

    pub fn diagonal_inches(&self) -> Option<f32> {
        let (w, h) = self.size_cm?;
        let diag_cm = ((w as f32).powi(2) + (h as f32).powi(2)).sqrt();
        Some(diag_cm / 2.54)
    }
}

pub fn extract(
    edid_data: Option<&[u8]>,
    serial_number: Option<&str>,
    serial: Option<u32>,
) -> EdidInfo {
    let mut out = EdidInfo::default();
    let mut edid_serial = None;

    if let Some(bytes) = edid_data {
        let parsed = edid::parse(bytes);
        if parsed.is_done() {
            let (_, e) = parsed.unwrap();

            let vendor: String = e.header.vendor.iter().collect();
            out.manufacturer = clean(&vendor);
            out.product_code = Some(e.header.product);
            out.edid_version = Some((e.header.version, e.header.revision));
            if e.header.week != 0 && e.header.week != 0xFF {
                out.week = Some(e.header.week);
            }
            out.year = Some(1990 + e.header.year as u16);
            if e.display.width != 0 && e.display.height != 0 {
                out.size_cm = Some((e.display.width, e.display.height));
            }

            for d in &e.descriptors {
                match d {
                    Descriptor::SerialNumber(s) => edid_serial = clean(s),
                    Descriptor::DetailedTiming(t)
                        if out.native.is_none()
                            && t.horizontal_active_pixels != 0
                            && t.vertical_active_lines != 0 =>
                    {
                        out.native = Some((t.horizontal_active_pixels, t.vertical_active_lines));
                    }
                    _ => {}
                }
            }

            log::debug!(
                "EDID decoded ({} bytes): vendor={} product=0x{:04X} numeric-serial={} \
                 mfg=W{:02}/{} v{}.{} size={}x{}cm native={} descriptor-serial={:?}",
                bytes.len(),
                vendor.trim(),
                e.header.product,
                e.header.serial,
                e.header.week,
                out.year.unwrap_or(0),
                e.header.version,
                e.header.revision,
                e.display.width,
                e.display.height,
                out.native.map_or_else(|| "?".into(), |(w, h)| format!("{w}x{h}")),
                edid_serial,
            );
        } else {
            log::warn!("EDID block present ({} bytes) but failed to parse", bytes.len());
        }
    }

    out.serial = serial_number
        .and_then(clean)
        .or(edid_serial)
        .or_else(|| serial.filter(|s| *s != 0).map(|s| s.to_string()));

    out
}

fn clean(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_prefers_display_info_string() {
        let e = extract(None, Some(" ABC123 "), Some(42));
        assert_eq!(e.serial.as_deref(), Some("ABC123"));
        assert!(!e.is_empty());
    }

    #[test]
    fn numeric_serial_used_when_no_string() {
        let e = extract(None, None, Some(42));
        assert_eq!(e.serial.as_deref(), Some("42"));
    }

    #[test]
    fn zero_numeric_serial_ignored() {
        assert!(extract(None, None, Some(0)).serial.is_none());
    }

    #[test]
    fn no_data_is_empty() {
        assert!(extract(None, None, None).is_empty());
    }

    #[test]
    fn diagonal_from_size() {
        let info = EdidInfo { size_cm: Some((60, 34)), ..Default::default() };
        let d = info.diagonal_inches().unwrap();
        assert!((d - 27.14).abs() < 0.1, "got {d}");
    }
}
