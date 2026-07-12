//! Custom title bar: draggable header row, window control buttons, and the
//! invisible edge/corner handles that let the undecorated window be resized.

use egui::{
    Align, CursorIcon, Id, Layout, PointerButton, Pos2, Rect, ResizeDirection, RichText, Sense,
    Stroke, Vec2, ViewportCommand,
};

use super::app::MplexerApp;
use super::theme::Palette;
use super::widgets;

const SCALE_OPTIONS: [u32; 5] = [50, 75, 100, 125, 150];
const RESIZE_BORDER: f32 = 6.0;

impl MplexerApp {
    pub(super) fn header(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // Drag anywhere on the bar background to move the (undecorated) window;
        // the widgets added afterwards sit on top and keep their own clicks.
        let drag = ui.interact(ui.max_rect(), Id::new("titlebar_drag"), Sense::click_and_drag());
        if drag.double_clicked() {
            toggle_maximize(ui.ctx());
        }
        if drag.drag_started_by(PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }

        ui.horizontal_centered(|ui| {
            let (logo, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
            ui.painter().rect_filled(logo, 9.0, p.accent);
            widgets::draw_monitor(ui.painter(), logo.shrink(4.5), p, p.on_accent);

            ui.add_space(6.0);
            ui.label(RichText::new("MPLEXER").color(p.text_strong).strong().size(25.0));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.vertical(|ui| {
                    // Deterministic row heights so the fixed header height matches.
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let width = ui.available_width();
                    ui.allocate_ui_with_layout(
                        Vec2::new(width, 24.0),
                        Layout::right_to_left(Align::Center),
                        |ui| window_controls(ui, p),
                    );
                    ui.add_space(8.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().interact_size.y = 24.0;
                        let toggle = if self.dark { "\u{25D0} DARK" } else { "\u{25D1} LIGHT" };
                        let btn = egui::Button::new(RichText::new(toggle).size(11.0))
                            .min_size(Vec2::new(0.0, 24.0));
                        if ui.add(btn).clicked() {
                            self.dark = !self.dark;
                            self.save_prefs();
                        }
                        ui.add_space(6.0);
                        if scale_selector(ui, &mut self.scale) {
                            self.save_prefs();
                        }
                        ui.add_space(10.0);

                        let online = self.state.peer_online || !self.state.hosts.is_empty();
                        let (dot, label) = if online {
                            (p.green, "SYSTEM ONLINE")
                        } else {
                            (p.muted, "SYSTEM IDLE")
                        };
                        ui.label(RichText::new(label).color(dot).size(11.0));
                        widgets::led(ui, dot);
                    });
                });
            });
        });
    }
}

/// Dropdown that scales the whole UI via egui's zoom factor. Shows the current
/// percentage; ratios are preserved because every point-sized element scales.
/// Returns `true` when the user picked a new value.
fn scale_selector(ui: &mut egui::Ui, scale: &mut f32) -> bool {
    let current = (*scale * 100.0).round() as u32;
    let mut changed = false;
    egui::ComboBox::from_id_salt("ui_scale")
        .width(46.0)
        .selected_text(RichText::new(format!("{current}%")).size(11.0))
        .show_ui(ui, |ui| {
            for pct in SCALE_OPTIONS {
                if ui.selectable_label(pct == current, format!("{pct}%")).clicked() {
                    *scale = pct as f32 / 100.0;
                    changed = true;
                }
            }
        });
    changed
}

#[derive(Clone, Copy)]
enum WinIcon {
    Minimize,
    Maximize,
    Close,
}

/// Minimize / maximize / close buttons for the custom title bar, laid out
/// right-to-left so the close button sits in the corner.
fn window_controls(ui: &mut egui::Ui, p: &Palette) {
    if win_button(ui, WinIcon::Close, p, true) {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
    if win_button(ui, WinIcon::Maximize, p, false) {
        toggle_maximize(ui.ctx());
    }
    if win_button(ui, WinIcon::Minimize, p, false) {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

/// A frameless title-bar button whose icon is drawn with the painter so it
/// never depends on font glyph coverage. `danger` tints the close hover red.
/// The button has a static logical size so it never stretches the header row.
fn win_button(ui: &mut egui::Ui, icon: WinIcon, p: &Palette, danger: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(32.0, 24.0), Sense::click());
    let hovered = resp.hovered();
    if hovered {
        let fill = if danger { p.red } else { p.card_hover };
        ui.painter().rect_filled(rect, 3.0, fill);
    }
    let color = if danger && hovered { p.on_accent } else { p.text };
    let c = rect.center();
    let s = 5.0;
    let stroke = Stroke::new(1.4, color);
    let painter = ui.painter();
    match icon {
        WinIcon::Minimize => {
            painter.line_segment([Pos2::new(c.x - s, c.y), Pos2::new(c.x + s, c.y)], stroke);
        }
        WinIcon::Maximize => {
            let glyph = Rect::from_center_size(c, Vec2::splat(s * 2.0));
            painter.rect_stroke(glyph, 0.0, stroke, egui::StrokeKind::Inside);
        }
        WinIcon::Close => {
            painter.line_segment([Pos2::new(c.x - s, c.y - s), Pos2::new(c.x + s, c.y + s)], stroke);
            painter.line_segment([Pos2::new(c.x - s, c.y + s), Pos2::new(c.x + s, c.y - s)], stroke);
        }
    }
    resp.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

fn toggle_maximize(ctx: &egui::Context) {
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
}

/// Invisible drag strips along the window edges/corners so the undecorated
/// window can still be resized. Each strip starts an OS-driven resize.
pub(super) fn resize_handles(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let b = RESIZE_BORDER;
    let handles = [
        (Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), rect.top() + b)), ResizeDirection::North, CursorIcon::ResizeVertical),
        (Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - b), rect.right_bottom()), ResizeDirection::South, CursorIcon::ResizeVertical),
        (Rect::from_min_max(Pos2::new(rect.right() - b, rect.top()), rect.right_bottom()), ResizeDirection::East, CursorIcon::ResizeHorizontal),
        (Rect::from_min_max(rect.left_top(), Pos2::new(rect.left() + b, rect.bottom())), ResizeDirection::West, CursorIcon::ResizeHorizontal),
        (Rect::from_min_max(Pos2::new(rect.right() - b, rect.bottom() - b), rect.right_bottom()), ResizeDirection::SouthEast, CursorIcon::ResizeNwSe),
        (Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - b), Pos2::new(rect.left() + b, rect.bottom())), ResizeDirection::SouthWest, CursorIcon::ResizeNeSw),
        (Rect::from_min_max(Pos2::new(rect.right() - b, rect.top()), Pos2::new(rect.right(), rect.top() + b)), ResizeDirection::NorthEast, CursorIcon::ResizeNeSw),
        (Rect::from_min_max(rect.left_top(), Pos2::new(rect.left() + b, rect.top() + b)), ResizeDirection::NorthWest, CursorIcon::ResizeNwSe),
    ];
    for (i, (hit, dir, cursor)) in handles.into_iter().enumerate() {
        let resp = ui.interact(hit, Id::new(("resize", i)), Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        if resp.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}
