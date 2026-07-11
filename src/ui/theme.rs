use egui::{Color32, FontFamily, Style, Visuals};

/// Flat color set for one theme variant. Kept as plain fields so the rest of the
/// UI can read named roles instead of scattering hex values.
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub field: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    pub muted: Color32,
    pub green: Color32,
    pub blue: Color32,
    pub red: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: Color32::from_rgb(0x0d, 0x12, 0x18),
            panel: Color32::from_rgb(0x10, 0x16, 0x1d),
            card: Color32::from_rgb(0x12, 0x1a, 0x22),
            card_hover: Color32::from_rgb(0x16, 0x20, 0x2a),
            border: Color32::from_rgb(0x24, 0x30, 0x3b),
            border_strong: Color32::from_rgb(0x35, 0x41, 0x4d),
            field: Color32::from_rgb(0x0a, 0x0f, 0x14),
            accent: Color32::from_rgb(0xf5, 0xa6, 0x23),
            on_accent: Color32::from_rgb(0x10, 0x0c, 0x02),
            text: Color32::from_rgb(0xc9, 0xd3, 0xdc),
            text_strong: Color32::from_rgb(0xe6, 0xed, 0xf3),
            muted: Color32::from_rgb(0x6b, 0x79, 0x86),
            green: Color32::from_rgb(0x3f, 0xb9, 0x50),
            blue: Color32::from_rgb(0x58, 0x92, 0xb0),
            red: Color32::from_rgb(0xf0, 0x53, 0x3f),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            bg: Color32::from_rgb(0xe9, 0xed, 0xf1),
            panel: Color32::from_rgb(0xfb, 0xfc, 0xfd),
            card: Color32::from_rgb(0xff, 0xff, 0xff),
            card_hover: Color32::from_rgb(0xf2, 0xf5, 0xf8),
            border: Color32::from_rgb(0xd3, 0xda, 0xe1),
            border_strong: Color32::from_rgb(0xb4, 0xbe, 0xc8),
            field: Color32::from_rgb(0xff, 0xff, 0xff),
            accent: Color32::from_rgb(0xe0, 0x92, 0x10),
            on_accent: Color32::from_rgb(0xff, 0xff, 0xff),
            text: Color32::from_rgb(0x35, 0x40, 0x4b),
            text_strong: Color32::from_rgb(0x12, 0x1a, 0x22),
            muted: Color32::from_rgb(0x74, 0x81, 0x8d),
            green: Color32::from_rgb(0x1f, 0x9d, 0x3a),
            blue: Color32::from_rgb(0x3d, 0x71, 0x8f),
            red: Color32::from_rgb(0xd6, 0x3a, 0x28),
        }
    }
}

/// Push the palette into egui's global style: colors, monospace text, and the
/// terminal-ish spacing the mockup uses.
pub fn apply(ctx: &egui::Context, p: &Palette) {
    let mut style = Style::default();

    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.override_text_color = Some(p.text);
    v.panel_fill = p.panel;
    v.window_fill = p.bg;
    v.faint_bg_color = p.card;
    v.extreme_bg_color = p.field;
    v.window_stroke = egui::Stroke::new(1.0, p.border);
    v.selection.bg_fill = p.accent.linear_multiply(0.35);
    v.selection.stroke = egui::Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;

    let radius = 4u8;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius.into();
        w.fg_stroke = egui::Stroke::new(1.0, p.text);
    }
    v.widgets.noninteractive.bg_fill = p.card;
    v.widgets.noninteractive.weak_bg_fill = p.card;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.border);
    v.widgets.inactive.bg_fill = p.card;
    v.widgets.inactive.weak_bg_fill = p.card;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, p.border);
    v.widgets.hovered.bg_fill = p.card_hover;
    v.widgets.hovered.weak_bg_fill = p.card_hover;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, p.border_strong);
    v.widgets.active.bg_fill = p.card_hover;
    v.widgets.active.weak_bg_fill = p.card_hover;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, p.accent);

    style.visuals = v;

    for id in style.text_styles.values_mut() {
        id.family = FontFamily::Monospace;
    }
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);

    ctx.all_styles_mut(move |s| *s = style.clone());
}
