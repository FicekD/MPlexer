//! The right-hand side panel: discovered network hosts and comms settings.

use egui::{Align, Align2, Frame, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::config::Config;
use crate::net::{HostStatus, HostView, NetCmd};

use super::app::MplexerApp;
use super::theme::Palette;
use super::widgets;

pub(super) const HOSTS_WIDTH: f32 = 306.0;

impl MplexerApp {
    pub(super) fn hosts_side(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::Panel::bottom("settings")
            .frame(Frame::default().fill(p.panel).inner_margin(Margin::same(14)))
            .show(ui, |ui| self.settings_pane(ui, p));

        egui::CentralPanel::default()
            .frame(Frame::default().fill(p.panel).inner_margin(Margin::same(14)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    widgets::accent_bar(ui, p);
                    ui.label(RichText::new("NETWORK HOSTS").color(p.text_strong).strong().size(13.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("LIVE").color(p.green).size(10.0));
                        widgets::led(ui, p.green);
                    });
                });
                ui.label(
                    RichText::new(format!("THIS: {}", self.config.machine_name))
                        .color(p.muted)
                        .size(10.0),
                );
                ui.add_space(6.0);
                divider(ui, p);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("DISCOVERED {} HOSTS", self.state.hosts.len()))
                            .color(p.muted)
                            .size(10.0),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button(RichText::new("\u{21BB} REFRESH").size(9.0))
                            .on_hover_text("Clear and rescan the LAN for hosts")
                            .clicked()
                        {
                            self.net.send(NetCmd::RefreshHosts);
                        }
                    });
                });
                ui.add_space(4.0);

                let mut toggled: Option<(uuid::Uuid, HostStatus)> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.state.hosts.is_empty() {
                        ui.add_space(10.0);
                        ui.label(RichText::new("NO HOSTS ON LAN").color(p.muted).size(11.0));
                    }
                    for host in &self.state.hosts {
                        if host_row(ui, host, p) {
                            toggled = Some((host.machine_id, host.status));
                        }
                    }
                });

                if let Some((id, status)) = toggled {
                    let cmd = if status == HostStatus::Connected {
                        NetCmd::SelectPeer(None)
                    } else {
                        NetCmd::SelectPeer(Some(id))
                    };
                    self.net.send(cmd);
                }
            });
    }

    fn settings_pane(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            widgets::accent_bar(ui, p);
            ui.label(RichText::new("COMMUNICATION PORT").color(p.text_strong).strong().size(12.0));
        });
        ui.add_space(8.0);
        ui.label(RichText::new("LISTEN / ADVERTISE").color(p.muted).size(10.0));
        ui.add_space(3.0);
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.port_buf)
                .hint_text("48669")
                .desired_width(f32::INFINITY)
                .margin(Margin::symmetric(8, 6)),
        );
        ui.add_space(8.0);

        let save = egui::Button::new(RichText::new("SAVE PORT").color(p.on_accent).strong().size(12.0))
            .fill(p.accent)
            .min_size(Vec2::new(0.0, 30.0));
        let clicked = ui.add_sized([ui.available_width(), 30.0], save).clicked();
        let submit = clicked
            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if submit {
            match self.port_buf.trim().parse::<u16>() {
                Ok(port) if port != 0 => {
                    self.config.comms_port = port;
                    self.state.status = match Config::persist_with(|c| c.comms_port = port) {
                        Ok(()) => format!("port {port} saved, restart to apply"),
                        Err(e) => format!("save failed: {e}"),
                    };
                }
                _ => self.state.status = "invalid port".to_string(),
            }
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("SAVED {} \u{00B7} APPLIES ON RESTART", self.config.comms_port))
                .color(p.muted)
                .size(9.0),
        );
    }
}

/// Draw one clickable host row; returns `true` when the user clicked it.
fn host_row(ui: &mut egui::Ui, host: &HostView, p: &Palette) -> bool {
    let (color, label) = match host.status {
        HostStatus::Connected => (p.accent, "CONNECTED"),
        HostStatus::Connecting => (p.blue, "CONNECTING"),
        HostStatus::Online => (p.green, "ONLINE"),
        HostStatus::Offline => (p.muted, "OFFLINE"),
    };
    let active = matches!(host.status, HostStatus::Connected | HostStatus::Connecting);

    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 50.0), Sense::click());
    let painter = ui.painter().clone();
    if active {
        painter.rect_filled(rect, 5.0, p.card);
    } else if resp.hovered() {
        painter.rect_filled(rect, 5.0, p.card_hover);
    }
    if active {
        let bar = Rect::from_min_size(rect.left_top(), Vec2::new(2.5, rect.height()));
        painter.rect_filled(bar, 0.0, color);
    }

    let left = rect.left() + 12.0;
    let top = rect.top() + 8.0;
    painter.circle_filled(Pos2::new(left, top + 4.0), 3.5, color);
    painter.text(
        Pos2::new(left + 12.0, top),
        Align2::LEFT_TOP,
        &host.name,
        FontId::monospace(12.0),
        p.text_strong,
    );
    painter.text(
        Pos2::new(rect.right() - 10.0, top),
        Align2::RIGHT_TOP,
        label,
        FontId::monospace(9.0),
        color,
    );
    painter.text(
        Pos2::new(left + 12.0, top + 16.0),
        Align2::LEFT_TOP,
        &host.addr,
        FontId::monospace(10.0),
        p.text,
    );
    let short = host.machine_id.simple().to_string();
    painter.text(
        Pos2::new(left + 12.0, top + 30.0),
        Align2::LEFT_TOP,
        format!("ID {}", &short[..8]),
        FontId::monospace(9.0),
        p.muted,
    );
    painter.line_segment(
        [
            Pos2::new(rect.left(), rect.bottom()),
            Pos2::new(rect.right(), rect.bottom()),
        ],
        Stroke::new(1.0, p.border),
    );

    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn divider(ui: &mut egui::Ui, p: &Palette) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
}
