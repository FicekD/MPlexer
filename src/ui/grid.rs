//! The central panel: monitor grid, per-card controls, and the summary bar.

use std::sync::mpsc::Sender;

use egui::{Align, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, Vec2};

use crate::config::Config;
use crate::ddc::{EdidInfo, InputSource};
use crate::model::MonitorState;
use crate::net::{NetCmd, NetHandle};
use crate::worker::WorkerCmd;

use super::app::MplexerApp;
use super::theme::Palette;
use super::widgets::{self, CardStatus};

const CARD_SIZE: Vec2 = Vec2::new(244.0, 252.0);
// Fixed heights for the two variable sections so every tile is the same height
// regardless of how much EDID a panel exposes or whether it's controllable.
// Sized generously to fit their fullest content (3 metadata rows / the combo).
const META_SLOT_H: f32 = 50.0;
const CONTROL_SLOT_H: f32 = 28.0;

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

        let config = &self.config;
        let cmd_tx = &self.cmd_tx;
        let net = &self.net;

        // Top-aligned wrap so every card's top edge lines up regardless of height.
        let layout = Layout::left_to_right(Align::Min).with_main_wrap(true);
        ui.with_layout(layout, |ui| {
            for (i, m) in self.state.monitors.iter_mut().enumerate() {
                ui.allocate_ui(CARD_SIZE, |ui| {
                    monitor_card(ui, i + 1, m, config, cmd_tx, net, p);
                });
            }
        });
    }

    fn summary_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let mut active = 0;
        let mut standby = 0;
        let mut error = 0;
        for m in &self.state.monitors {
            match CardStatus::derive(
                m.reachable(),
                m.switchable_here() || m.switchable_by_peer(),
                m.pending,
            ) {
                CardStatus::Active => active += 1,
                CardStatus::NoControl | CardStatus::Pending => standby += 1,
                CardStatus::Offline => error += 1,
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

fn monitor_card(
    ui: &mut egui::Ui,
    index: usize,
    m: &mut MonitorState,
    config: &Config,
    cmd_tx: &Sender<WorkerCmd>,
    net: &NetHandle,
    p: &Palette,
) {
    let status = CardStatus::derive(
        m.reachable(),
        m.switchable_here() || m.switchable_by_peer(),
        m.pending,
    );
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

                edid_details(ui, &m.edid, p);

                ui.add_space(4.0);
                ui.label(RichText::new("INPUT SOURCE").color(p.muted).size(9.0));
                ui.add_space(2.0);

                let width = ui.available_width();
                let chosen = ui
                    .allocate_ui(Vec2::new(width, CONTROL_SLOT_H), |ui| {
                        ui.set_min_height(CONTROL_SLOT_H);
                        if !m.switchable_here() && !m.switchable_by_peer() {
                            ui.label(RichText::new("NO CONTROL").color(p.blue).size(10.0))
                                .on_hover_text(
                                    "This display doesn't answer DDC/CI, so its input can't be switched from software.",
                                );
                            return None;
                        }
                        input_selector(ui, m, config, p)
                    })
                    .inner;

                if let Some(chosen) = chosen
                    && Some(chosen) != m.shown_input()
                    && !m.pending
                {
                    m.pending = true;
                    // Attempt locally and ask a peer cabled to this monitor to do the
                    // same; whichever host the panel still listens to makes it stick.
                    if m.switchable_here() {
                        cmd_tx.send(WorkerCmd::SetInput { id: m.id.clone(), input: chosen }).ok();
                    }
                    if m.switchable_by_peer() {
                        net.send(NetCmd::RequestSwitch { id: m.id.clone(), target: chosen });
                    }
                }
            });
        });
}

/// The per-monitor input dropdown, listing only inputs confirmed by observation.
/// Returns a newly picked input, if any.
fn input_selector(
    ui: &mut egui::Ui,
    m: &MonitorState,
    config: &Config,
    p: &Palette,
) -> Option<InputSource> {
    let inputs = m.inputs();
    if inputs.is_empty() {
        ui.label(RichText::new("DETECTING\u{2026}").color(p.muted).size(10.0)).on_hover_text(
            "No input seen yet. Switch this display once (its own button works) and it'll appear here.",
        );
        return None;
    }

    let current = m.shown_input();
    let mut selected = current;
    ui.add_enabled_ui(!m.pending, |ui| {
        egui::ComboBox::from_id_salt(&m.id)
            .width(ui.available_width() - 8.0)
            .selected_text(RichText::new(selected_input_label(current, config)).size(11.0))
            .show_ui(ui, |ui| {
                for &inp in &inputs {
                    ui.selectable_value(&mut selected, Some(inp), short_label(inp, config));
                }
            });
    });
    selected.filter(|&inp| Some(inp) != current)
}

/// Serial number and the EDID metadata worth showing per tile: physical size,
/// native resolution, manufacture date, and EDID version. Rendered into a
/// fixed-height slot (top-aligned) so tiles stay the same height whether the
/// panel exposes a full EDID or none at all.
fn edid_details(ui: &mut egui::Ui, edid: &EdidInfo, p: &Palette) {
    ui.add_space(4.0);
    ui.allocate_ui(Vec2::new(ui.available_width(), META_SLOT_H), |ui| {
        ui.set_min_height(META_SLOT_H);
        ui.spacing_mut().item_spacing.y = 2.0;

        if edid.is_empty() {
            ui.label(RichText::new("NO EDID").color(p.muted).monospace().size(9.0))
                .on_hover_text("This display didn't report EDID metadata.");
            return;
        }

        if let Some(serial) = &edid.serial {
            meta_row(ui, p, "SN", serial);
        }

        let mut panel = Vec::new();
        if let Some(d) = edid.diagonal_inches() {
            panel.push(format!("{d:.0}\""));
        }
        if let Some((w, h)) = edid.native {
            panel.push(format!("{w}\u{00D7}{h}"));
        }
        if !panel.is_empty() {
            meta_row(ui, p, "PANEL", &panel.join(" "));
        }

        let mut mfg = Vec::new();
        if let Some(year) = edid.year {
            mfg.push(match edid.week {
                Some(w) => format!("W{w:02} {year}"),
                None => year.to_string(),
            });
        }
        if let Some((v, r)) = edid.edid_version {
            mfg.push(format!("EDID {v}.{r}"));
        }
        if !mfg.is_empty() {
            meta_row(ui, p, "MFG", &mfg.join(" "));
        }
    });
}

/// One `TAG  value` line of monitor metadata. Tag and value share a font size so
/// their baselines line up; the value truncates and reveals in full on hover so
/// long serials don't blow out the card width.
fn meta_row(ui: &mut egui::Ui, p: &Palette, tag: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(tag).color(p.muted).monospace().size(9.0));
        ui.add(
            egui::Label::new(RichText::new(value).color(p.text).monospace().size(9.0)).truncate(),
        )
        .on_hover_text(value);
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

/// Input label for the dropdown: the raw hex code, unless the user gave it a
/// custom name in `input_labels` (keyed by the `0x..` code).
fn short_label(input: InputSource, config: &Config) -> String {
    let code = format!("0x{:02X}", input.0);
    match config.input_labels.get(&code) {
        Some(custom) => custom.to_uppercase(),
        None => code,
    }
}
