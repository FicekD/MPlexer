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
                let count = self.log_buffer.lock().unwrap().len();
                ui.label(RichText::new(format!("{count} LINES")).color(p.muted).size(10.0));
            });
        });
        ui.add_space(4.0);

        Frame::default()
            .fill(p.field)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(5)
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                let lines = self.log_buffer.lock().unwrap().clone();
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
