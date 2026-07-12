//! The collapsible log console shown at the bottom of the central panel.

use egui::{Frame, Layout, Align, Margin, RichText, Stroke};

use super::app::MplexerApp;
use super::theme::Palette;
use super::widgets;

impl MplexerApp {
    pub(super) fn log_console(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            widgets::accent_bar(ui, p);
            ui.label(RichText::new("LOG CONSOLE").color(p.text_strong).strong().size(12.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(RichText::new("CLEAR").size(10.0)).clicked() {
                    self.log_buffer.lock().unwrap().clear();
                }
                level_filter(ui, &mut self.log_level);
            });
        });
        ui.add_space(4.0);

        // The buffer holds everything down to DEBUG; the dropdown filters which
        // severities are shown (default INFO and above).
        let level = self.log_level;
        let lines: Vec<_> = self
            .log_buffer
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.level <= level)
            .cloned()
            .collect();

        Frame::default()
            .fill(p.field)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(5)
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        if lines.is_empty() {
                            ui.label(RichText::new("no log output yet").color(p.muted).size(11.0));
                        }
                        for line in &lines {
                            log_row(ui, line, p);
                        }
                    });
            });
    }
}

/// Dropdown selecting the minimum severity shown in the console. Levels are
/// ordered Error < Warn < Info < Debug, so a line is shown when its level is at
/// or above the chosen threshold. Built from a plain button (matching CLEAR's
/// height and alignment) plus a menu popup, so there's no combo scrollbar.
fn level_filter(ui: &mut egui::Ui, level: &mut log::Level) {
    let response = ui.button(RichText::new(format!("{level} \u{25BE}")).size(10.0));
    egui::Popup::menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            for option in [
                log::Level::Error,
                log::Level::Warn,
                log::Level::Info,
                log::Level::Debug,
            ] {
                if ui
                    .selectable_label(*level == option, RichText::new(option.to_string()).size(10.0))
                    .clicked()
                {
                    *level = option;
                }
            }
        });
}

/// One monospace log line, level-colored, for the console.
fn log_row(ui: &mut egui::Ui, line: &crate::logging::LogLine, p: &Palette) {
    let color = match line.level {
        log::Level::Error => p.red,
        log::Level::Warn => p.accent,
        log::Level::Info => p.text,
        log::Level::Debug | log::Level::Trace => p.muted,
    };
    let target = line.target.strip_prefix("mplexer::").unwrap_or(&line.target);
    let text = format!("{} {:<5} {} - {}", line.time, line.level, target, line.msg);
    ui.add(egui::Label::new(RichText::new(text).color(color).size(11.0)).wrap());
}
