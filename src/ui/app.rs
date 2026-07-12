use std::sync::mpsc::{Receiver, Sender};

use egui::{Frame, Margin};

use crate::config::Config;
use crate::logging::LogBuffer;
use crate::model::AppState;
use crate::net::{NetCmd, NetEvt, NetHandle};
use crate::worker::{WorkerCmd, WorkerEvt};

use super::events::{apply_net_event, apply_set_result, merge_snapshots};
use super::hosts::HOSTS_WIDTH;
use super::theme::{self, Palette};
use super::titlebar::resize_handles;

pub struct MplexerApp {
    pub(super) state: AppState,
    pub(super) cmd_tx: Sender<WorkerCmd>,
    pub(super) evt_rx: Receiver<WorkerEvt>,
    pub(super) net: NetHandle,
    pub(super) net_evt_rx: Receiver<NetEvt>,
    pub(super) config: Config,
    pub(super) dark: bool,
    pub(super) theme_applied: Option<bool>,
    pub(super) scale: f32,
    pub(super) port_buf: String,
    pub(super) log_buffer: LogBuffer,
    pub(super) show_log: bool,
}

impl MplexerApp {
    pub fn new(
        config: Config,
        cmd_tx: Sender<WorkerCmd>,
        evt_rx: Receiver<WorkerEvt>,
        net: NetHandle,
        net_evt_rx: Receiver<NetEvt>,
        log_buffer: LogBuffer,
    ) -> Self {
        let port_buf = config.comms_port.to_string();
        let dark = config.dark_theme;
        let scale = config.ui_scale as f32 / 100.0;
        Self {
            state: AppState::default(),
            cmd_tx,
            evt_rx,
            net,
            net_evt_rx,
            config,
            dark,
            theme_applied: None,
            scale,
            port_buf,
            log_buffer,
            show_log: false,
        }
    }

    /// Mirror the current theme/scale choices into the config file so they
    /// survive a restart. Called only when the user changes them.
    pub(super) fn save_prefs(&mut self) {
        self.config.dark_theme = self.dark;
        self.config.ui_scale = (self.scale * 100.0).round() as u32;
        let (dark, scale) = (self.dark, self.config.ui_scale);
        if let Err(e) = Config::persist_with(|c| {
            c.dark_theme = dark;
            c.ui_scale = scale;
        }) {
            log::warn!("failed to persist UI preferences: {e:#}");
        }
    }
}

impl eframe::App for MplexerApp {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(evt) = self.evt_rx.try_recv() {
            match evt {
                WorkerEvt::Monitors(list) => {
                    self.net.send(NetCmd::LocalState(list.clone()));
                    merge_snapshots(&mut self.state.monitors, list);
                }
                WorkerEvt::SetResult { id, result } => {
                    self.net.send(NetCmd::LocalSetResult {
                        id: id.clone(),
                        result: result.clone(),
                    });
                    apply_set_result(&mut self.state, id, result);
                }
            }
        }

        while let Ok(evt) = self.net_evt_rx.try_recv() {
            apply_net_event(&mut self.state, &self.cmd_tx, evt);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = if self.dark { Palette::dark() } else { Palette::light() };
        if self.theme_applied != Some(self.dark) {
            theme::apply(ui.ctx(), &p);
            self.theme_applied = Some(self.dark);
        }
        ui.ctx().set_zoom_factor(self.scale);

        resize_handles(ui);

        // Fixed header height (both button rows have static sizes) so the
        // left brand can be vertically centered across both rows.
        const HEADER_CONTENT: f32 = 24.0 + 8.0 + 24.0;
        egui::Panel::top("header")
            .exact_size(HEADER_CONTENT + 24.0)
            .frame(Frame::default().fill(p.panel).inner_margin(Margin::symmetric(16, 10)))
            .show(ui, |ui| self.header(ui, &p));

        egui::Panel::right("hosts")
            .resizable(true)
            .default_size(HOSTS_WIDTH)
            .min_size(240.0)
            .max_size(560.0)
            .frame(Frame::default().fill(p.panel))
            .show(ui, |ui| self.hosts_side(ui, &p));

        egui::CentralPanel::default()
            .frame(Frame::default().fill(p.bg))
            .show(ui, |ui| self.central(ui, &p));
    }
}
