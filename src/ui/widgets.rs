use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, Vec2};

use super::theme::Palette;
use crate::model::Owner;

/// Coarse per-monitor state shown as the card's status badge, derived from the
/// real availability/owner/pending flags.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CardStatus {
    Active,
    Standby,
    Error,
    Pending,
    Unknown,
}

impl CardStatus {
    pub fn derive(available: bool, owner: Owner, pending: bool) -> Self {
        if pending {
            return CardStatus::Pending;
        }
        match owner {
            Owner::Me if available => CardStatus::Active,
            Owner::Me => CardStatus::Error,
            Owner::Peer => CardStatus::Standby,
            Owner::Unknown if available => CardStatus::Active,
            Owner::Unknown => CardStatus::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CardStatus::Active => "ACTIVE",
            CardStatus::Standby => "STANDBY",
            CardStatus::Error => "ERROR",
            CardStatus::Pending => "PENDING",
            CardStatus::Unknown => "UNKNOWN",
        }
    }

    pub fn color(self, p: &Palette) -> Color32 {
        match self {
            CardStatus::Active => p.green,
            CardStatus::Standby => p.blue,
            CardStatus::Error => p.red,
            CardStatus::Pending => p.accent,
            CardStatus::Unknown => p.muted,
        }
    }
}

/// Draw a small filled status dot inline and advance the cursor.
pub fn led(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(9.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
    ui.painter()
        .circle_stroke(rect.center(), 4.5, Stroke::new(1.0, color.linear_multiply(0.4)));
}

/// Paint a stylized monitor glyph filling `rect`, tinted by `accent`.
pub fn draw_monitor(painter: &egui::Painter, rect: Rect, p: &Palette, accent: Color32) {
    let w = rect.width();
    let h = rect.height();
    let screen = Rect::from_min_size(
        rect.min + Vec2::new(w * 0.12, h * 0.10),
        Vec2::new(w * 0.76, h * 0.56),
    );
    painter.rect_filled(screen, 3.0, p.field);
    painter.rect_stroke(screen, 3.0, Stroke::new(1.6, accent), StrokeKind::Inside);

    // Scanline hint inside the screen.
    let y = screen.center().y;
    painter.line_segment(
        [
            Pos2::new(screen.left() + w * 0.08, y),
            Pos2::new(screen.right() - w * 0.08, y),
        ],
        Stroke::new(1.0, accent.linear_multiply(0.5)),
    );

    // Neck + base of the stand.
    let neck_top = screen.bottom();
    let cx = rect.center().x;
    painter.line_segment(
        [Pos2::new(cx, neck_top), Pos2::new(cx, neck_top + h * 0.12)],
        Stroke::new(2.0, p.border_strong),
    );
    let base_y = neck_top + h * 0.14;
    painter.line_segment(
        [
            Pos2::new(cx - w * 0.18, base_y),
            Pos2::new(cx + w * 0.18, base_y),
        ],
        Stroke::new(2.4, p.border_strong),
    );
}
