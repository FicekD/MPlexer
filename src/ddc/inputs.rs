#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct InputSource(pub u8);

impl InputSource {
    pub fn label(self) -> String {
        match self.0 {
            0x01 => "VGA-1".to_string(),
            0x03 => "DVI-1".to_string(),
            0x04 => "DVI-2".to_string(),
            0x0C => "Component-1".to_string(),
            0x0F => "DisplayPort-1".to_string(),
            0x10 => "DisplayPort-2".to_string(),
            0x11 => "HDMI-1".to_string(),
            0x12 => "HDMI-2".to_string(),
            other => format!("Input 0x{other:02X}"),
        }
    }
}

pub fn parse_input_list(caps: &str) -> Vec<InputSource> {
    let lower = caps.to_lowercase();

    let Some(vcp_open) = lower.find("vcp(") else {
        return Vec::new();
    };
    let vcp_body_start = vcp_open + "vcp(".len();
    let Some(vcp_body_end) = matching_close_paren(&lower, vcp_body_start) else {
        return Vec::new();
    };
    let vcp_body = &lower[vcp_body_start..vcp_body_end];

    let Some(sixty_open) = vcp_body.find("60(") else {
        return Vec::new();
    };
    let sixty_body_start = sixty_open + "60(".len();
    let Some(sixty_body_end) = matching_close_paren(vcp_body, sixty_body_start) else {
        return Vec::new();
    };

    vcp_body[sixty_body_start..sixty_body_end]
        .split_whitespace()
        .filter_map(|token| u8::from_str_radix(token, 16).ok())
        .map(InputSource)
        .collect()
}

fn matching_close_paren(s: &str, start: usize) -> Option<usize> {
    let mut depth = 1u32;
    for (offset, ch) in s[start..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + offset);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_input_list_from_caps_string() {
        let caps = "(prot(monitor)type(lcd) vcp(02 04 60(11 12 0F) 62 ...) ...)";
        assert_eq!(
            parse_input_list(caps),
            vec![InputSource(0x11), InputSource(0x12), InputSource(0x0F)]
        );
    }

    #[test]
    fn missing_60_group_returns_empty() {
        let caps = "(prot(monitor)type(lcd) vcp(02 04 62) ...)";
        assert_eq!(parse_input_list(caps), Vec::new());
    }

    #[test]
    fn missing_vcp_group_returns_empty() {
        assert_eq!(parse_input_list("(prot(monitor)type(lcd))"), Vec::new());
    }

    #[test]
    fn tolerant_of_case_and_whitespace() {
        let caps = "(PROT(monitor)  VCP( 02   60( 11   12  0f )  62 ) )";
        assert_eq!(
            parse_input_list(caps),
            vec![InputSource(0x11), InputSource(0x12), InputSource(0x0F)]
        );
    }

    #[test]
    fn labels_match_mccs_table() {
        assert_eq!(InputSource(0x01).label(), "VGA-1");
        assert_eq!(InputSource(0x03).label(), "DVI-1");
        assert_eq!(InputSource(0x04).label(), "DVI-2");
        assert_eq!(InputSource(0x0C).label(), "Component-1");
        assert_eq!(InputSource(0x0F).label(), "DisplayPort-1");
        assert_eq!(InputSource(0x10).label(), "DisplayPort-2");
        assert_eq!(InputSource(0x11).label(), "HDMI-1");
        assert_eq!(InputSource(0x12).label(), "HDMI-2");
        assert_eq!(InputSource(0xAB).label(), "Input 0xAB");
    }
}
