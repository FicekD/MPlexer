//! The central panel: monitor grid, per-card controls, and the summary bar.

use std::sync::mpsc::Sender;

use egui::{Align, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, Vec2};

use crate::config::Config;
use crate::ddc::InputSource;
use crate::model::{MonitorState, Owner};
use crate::net::{NetCmd, NetHandle};
use crate::worker::WorkerCmd;

use super::app::MplexerApp;
use super::theme::Palette;
use super::widgets::{self, CardStatus};

const CARD_SIZE: Vec2 = Vec2::new(244.0, 230.0);

impl MplexerApp {
    pub(super) fn central(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::Panel::top("grid_header")
            .frame(Frame::default().fill(p.bg).inner_margin(Margin::symmetric(14, 12)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    widgets::accent_bar(ui, p);
                    ui.label(RichText::new("MONITORS").color(p.text_strong).strong().size(13.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(RichText::new("\u{21BB} REFRESH").size(11.0)).clicked() {
                            self.cmd_tx.send(WorkerCmd::Refresh).ok();
                        }
                        ui.add_space(6.0);
                        let log_label = RichText::new("\u{2261} LOG")
                            .size(11.0)
                            .color(if self.show_log { p.accent } else { p.muted });
                        if ui.selectable_label(self.show_log, log_label).clicked() {
                            self.show_log = !self.show_log;
                        }
                    });
                });
            });

        egui::Panel::bottom("summary")
            .frame(Frame::default().fill(p.bg).inner_margin(Margin::symmetric(14, 10)))
            .show(ui, |ui| self.summary_bar(ui, p));

        if self.show_log {
            egui::Panel::bottom("log_console")
                .resizable(true)
                .default_size(180.0)
                .min_size(80.0)
                .max_size(420.0)
                .frame(Frame::default().fill(p.bg).inner_margin(Margin::symmetric(14, 8)))
                .show(ui, |ui| self.log_console(ui, p));
        }

        egui::CentralPanel::default()
            .frame(Frame::default().fill(p.bg))
            .show(ui, |ui| {
                // Full-width scroll area so the scrollbar sits at the panel edge
                // instead of hugging the last column; the grid is inset via margin.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        Frame::default()
                            .inner_margin(Margin::symmetric(14, 4))
                            .show(ui, |ui| self.grid(ui, p));
                    });
            });
    }

    fn grid(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if self.state.monitors.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("SCANNING FOR MONITORS\u{2026}").color(p.muted).size(12.0));
            });
            return;
        }

        let peer_name = self.state.peer_name.clone();
        let config = &self.config;
        let cmd_tx = &self.cmd_tx;
        let net = &self.net;
        let peer_online = self.state.peer_online;

        // Top-aligned wrap so every card's top edge lines up regardless of height.
        let layout = Layout::left_to_right(Align::Min).with_main_wrap(true);
        ui.with_layout(layout, |ui| {
            for (i, m) in self.state.monitors.iter_mut().enumerate() {
                ui.allocate_ui(CARD_SIZE, |ui| {
                    monitor_card(
                        ui,
                        i + 1,
                        m,
                        config,
                        cmd_tx,
                        net,
                        peer_online,
                        peer_name.as_deref(),
                        p,
                    );
                });
            }
        });
    }

    fn summary_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let mut active = 0;
        let mut standby = 0;
        let mut error = 0;
        for m in &self.state.monitors {
            match CardStatus::derive(m.available, m.owner, m.pending) {
                CardStatus::Active => active += 1,
                CardStatus::Standby | CardStatus::Pending => standby += 1,
                CardStatus::Error | CardStatus::Unknown => error += 1,
            }
        }
        let total = self.state.monitors.len();

        Frame::default()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(5)
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    count_chip(ui, p, p.green, "ACTIVE", active);
                    count_chip(ui, p, p.blue, "STANDBY", standby);
                    count_chip(ui, p, p.red, "ERROR", error);
                    ui.label(RichText::new("|").color(p.border_strong));
                    ui.label(RichText::new(format!("TOTAL {total}")).color(p.muted).size(11.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let status = if self.state.status.is_empty() {
                            "READY".to_string()
                        } else {
                            self.state.status.to_uppercase()
                        };
                        ui.label(RichText::new(status).color(p.muted).size(11.0));
                    });
                });
            });
    }
}

#[allow(clippy::too_many_arguments)]
fn monitor_card(
    ui: &mut egui::Ui,
    index: usize,
    m: &mut MonitorState,
    config: &Config,
    cmd_tx: &Sender<WorkerCmd>,
    net: &NetHandle,
    peer_online: bool,
    peer_name: Option<&str>,
    p: &Palette,
) {
    let status = CardStatus::derive(m.available, m.owner, m.pending);
    Frame::default()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(6)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_size(CARD_SIZE - Vec2::splat(24.0));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("MONITOR {index:02}"))
                            .color(p.muted)
                            .size(10.0),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(status.label()).color(status.color(p)).size(10.0));
                        widgets::led(ui, status.color(p));
                    });
                });

                let (row, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 72.0), Sense::hover());
                let icon = Rect::from_center_size(row.center(), Vec2::new(94.0, 62.0));
                widgets::draw_monitor(ui.painter(), icon, p, status.color(p));

                ui.add(
                    egui::Label::new(RichText::new(&m.name).color(p.text_strong).size(12.0))
                        .truncate(),
                );
                let owner_label = match m.owner {
                    Owner::Me => format!("\u{25CF} {}", config.machine_name),
                    Owner::Peer => format!("\u{25CF} {}", peer_name.unwrap_or("peer")),
                    Owner::Unknown => "\u{25CB} unassigned".to_string(),
                };
                ui.add(egui::Label::new(RichText::new(owner_label).color(p.muted).size(10.0)).truncate());

                ui.add_space(4.0);
                ui.label(RichText::new("INPUT SOURCE").color(p.muted).size(9.0));
                ui.add_space(2.0);

                let route = route_for(m.owner, m.available, peer_online);
                let enabled = route != Route::Disabled && !m.pending;
                let current = m.current_input;
                let mut chosen = None;

                if m.valid_inputs.is_empty() {
                    ui.label(RichText::new("NO INPUTS").color(p.muted).size(10.0));
                } else {
                    let mut selected = current;
                    ui.add_enabled_ui(enabled, |ui| {
                        egui::ComboBox::from_id_salt(&m.id)
                            .width(ui.available_width() - 8.0)
                            .selected_text(
                                RichText::new(selected_input_label(current, config)).size(11.0),
                            )
                            .show_ui(ui, |ui| {
                                for &inp in &m.valid_inputs {
                                    ui.selectable_value(
                                        &mut selected,
                                        Some(inp),
                                        short_label(inp, config),
                                    );
                                }
                            });
                    });
                    if selected != current
                        && let Some(inp) = selected
                    {
                        chosen = Some(inp);
                    }
                }

                if route == Route::Disabled {
                    ui.label(RichText::new("PEER OFFLINE").color(p.red).size(9.0)).on_hover_text(
                        "This monitor is switched away and the peer is not connected.",
                    );
                }

                if let Some(inp) = chosen
                    && Some(inp) != current
                    && !m.pending
                {
                    match route {
                        Route::Local => {
                            m.pending = true;
                            cmd_tx
                                .send(WorkerCmd::SetInput { id: m.id.clone(), input: inp })
                                .ok();
                        }
                        Route::Peer => {
                            m.pending = true;
                            net.send(NetCmd::RequestSwitch { id: m.id.clone(), target: inp });
                        }
                        Route::Disabled => {}
                    }
                }
            });
        });
}

fn count_chip(ui: &mut egui::Ui, p: &Palette, color: egui::Color32, label: &str, n: usize) {
    widgets::led(ui, color);
    ui.label(RichText::new(label).color(p.muted).size(11.0));
    ui.label(RichText::new(n.to_string()).color(p.text_strong).strong().size(11.0));
    ui.add_space(6.0);
}

fn selected_input_label(current: Option<InputSource>, config: &Config) -> String {
    match current {
        Some(inp) => short_label(inp, config),
        None => "None".to_string(),
    }
}

/// Compact input name for the input dropdown; honours custom labels.
fn short_label(input: InputSource, config: &Config) -> String {
    let code = format!("0x{:02X}", input.0);
    if let Some(custom) = config.input_labels.get(&code) {
        return custom.to_uppercase();
    }
    match input.0 {
        0x01 => "VGA".to_string(),
        0x03 => "DVI-1".to_string(),
        0x04 => "DVI-2".to_string(),
        0x0F => "DP-1".to_string(),
        0x10 => "DP-2".to_string(),
        0x11 => "HDMI-1".to_string(),
        0x12 => "HDMI-2".to_string(),
        other => format!("0x{other:02X}"),
    }
}

/// Where a dropdown change for a monitor is routed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Route {
    /// We can write it ourselves (phase-2 path).
    Local,
    /// Ask the peer, which currently owns it, to switch it.
    Peer,
    /// Nobody can act on it right now.
    Disabled,
}

fn route_for(owner: Owner, available: bool, peer_online: bool) -> Route {
    match owner {
        Owner::Me => Route::Local,
        Owner::Peer if peer_online => Route::Peer,
        Owner::Peer => Route::Disabled,
        Owner::Unknown if available => Route::Local,
        Owner::Unknown if peer_online => Route::Peer,
        Owner::Unknown => Route::Disabled,
    }
}
