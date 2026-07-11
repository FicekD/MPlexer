mod theme;
mod widgets;

use std::sync::mpsc::{Receiver, Sender};

use egui::{Align, Align2, FontId, Frame, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::config::Config;
use crate::ddc::{InputSource, StableId};
use crate::model::{AppState, MonitorState, Owner};
use crate::net::{HostStatus, HostView, NetCmd, NetEvt, NetHandle};
use crate::worker::{MonitorSnapshot, WorkerCmd, WorkerEvt};
use theme::Palette;
use widgets::CardStatus;

const CARD_SIZE: Vec2 = Vec2::new(244.0, 230.0);
const HOSTS_WIDTH: f32 = 306.0;

pub struct MplexerApp {
    state: AppState,
    cmd_tx: Sender<WorkerCmd>,
    evt_rx: Receiver<WorkerEvt>,
    net: NetHandle,
    net_evt_rx: Receiver<NetEvt>,
    config: Config,
    dark: bool,
    theme_applied: Option<bool>,
    port_buf: String,
}

impl MplexerApp {
    pub fn new(
        config: Config,
        cmd_tx: Sender<WorkerCmd>,
        evt_rx: Receiver<WorkerEvt>,
        net: NetHandle,
        net_evt_rx: Receiver<NetEvt>,
    ) -> Self {
        let port_buf = config.comms_port.to_string();
        Self {
            state: AppState::default(),
            cmd_tx,
            evt_rx,
            net,
            net_evt_rx,
            config,
            dark: true,
            theme_applied: None,
            port_buf,
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

        egui::Panel::top("header")
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

impl MplexerApp {
    fn header(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            let (logo, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
            ui.painter().rect_filled(logo, 6.0, p.accent);
            widgets::draw_monitor(ui.painter(), logo.shrink(3.0), p, p.on_accent);

            ui.add_space(4.0);
            ui.label(RichText::new("MPLEXER").color(p.text_strong).strong().size(17.0));
            ui.add_space(6.0);
            ui.label(RichText::new("| MONITOR CONTROL").color(p.muted).size(11.0));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let toggle = if self.dark { "\u{25D0} DARK" } else { "\u{25D1} LIGHT" };
                if ui.button(RichText::new(toggle).size(11.0)).clicked() {
                    self.dark = !self.dark;
                }
                ui.add_space(10.0);

                let online = self.state.peer_online || !self.state.hosts.is_empty();
                let (dot, label) = if online {
                    (p.green, "SYS ONLINE")
                } else {
                    (p.muted, "SYS IDLE")
                };
                ui.label(RichText::new(label).color(dot).size(11.0));
                widgets::led(ui, dot);
            });
        });
    }

    fn central(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::Panel::top("grid_header")
            .frame(Frame::default().fill(p.bg).inner_margin(Margin::symmetric(14, 12)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    accent_bar(ui, p);
                    ui.label(RichText::new("MONITOR GRID").color(p.text_strong).strong().size(13.0));
                    ui.label(RichText::new("\u{2014} INPUT CONTROL").color(p.muted).size(11.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(RichText::new("\u{21BB} REFRESH").size(11.0)).clicked() {
                            self.cmd_tx.send(WorkerCmd::Refresh).ok();
                        }
                    });
                });
            });

        egui::Panel::bottom("summary")
            .frame(Frame::default().fill(p.bg).inner_margin(Margin::symmetric(14, 10)))
            .show(ui, |ui| self.summary_bar(ui, p));

        egui::CentralPanel::default()
            .frame(Frame::default().fill(p.bg).inner_margin(Margin::symmetric(14, 4)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.grid(ui, p));
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

    fn hosts_side(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::Panel::bottom("settings")
            .frame(Frame::default().fill(p.panel).inner_margin(Margin::same(14)))
            .show(ui, |ui| self.settings_pane(ui, p));

        egui::CentralPanel::default()
            .frame(Frame::default().fill(p.panel).inner_margin(Margin::same(14)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    accent_bar(ui, p);
                    ui.label(RichText::new("NETWORK HOSTS").color(p.text_strong).strong().size(13.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("LIVE").color(p.green).size(10.0));
                        widgets::led(ui, p.green);
                    });
                });
                ui.label(
                    RichText::new(format!("THIS \u{2014} {}", self.config.machine_name))
                        .color(p.muted)
                        .size(10.0),
                );
                ui.add_space(6.0);
                divider(ui, p);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("DISCOVERED \u{2014} {} HOSTS", self.state.hosts.len()))
                            .color(p.muted)
                            .size(10.0),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("CLICK TO LINK").color(p.muted).size(9.0));
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
            accent_bar(ui, p);
            ui.label(RichText::new("COMMS PORT").color(p.text_strong).strong().size(12.0));
        });
        ui.add_space(8.0);
        ui.label(RichText::new("LISTEN / ADVERTISE").color(p.muted).size(10.0));
        ui.add_space(3.0);
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.port_buf)
                .hint_text("48620")
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
                    self.state.status = match self.config.save() {
                        Ok(()) => format!("port {port} saved \u{2014} restart to apply"),
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

fn selected_input_label(current: Option<InputSource>, config: &Config) -> String {
    match current {
        Some(inp) => short_label(inp, config),
        None => "\u{2014}".to_string(),
    }
}

fn count_chip(ui: &mut egui::Ui, p: &Palette, color: egui::Color32, label: &str, n: usize) {
    widgets::led(ui, color);
    ui.label(RichText::new(label).color(p.muted).size(11.0));
    ui.label(RichText::new(n.to_string()).color(p.text_strong).strong().size(11.0));
    ui.add_space(6.0);
}

fn accent_bar(ui: &mut egui::Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(3.0, 15.0), Sense::hover());
    ui.painter().rect_filled(rect, 1.0, p.accent);
    ui.add_space(2.0);
}

fn divider(ui: &mut egui::Ui, p: &Palette) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
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

fn apply_net_event(state: &mut AppState, cmd_tx: &Sender<WorkerCmd>, evt: NetEvt) {
    match evt {
        NetEvt::PeerOnline { name } => {
            state.peer_online = true;
            state.peer_name = Some(name);
        }
        NetEvt::PeerOffline => state.peer_online = false,
        NetEvt::Hosts(hosts) => state.hosts = hosts,
        NetEvt::Owners(owners) => {
            for m in &mut state.monitors {
                m.owner = owners.get(&m.id).copied().unwrap_or(Owner::Unknown);
            }
        }
        NetEvt::ExecuteSwitch { id, target } => {
            if let Some(m) = state.monitors.iter_mut().find(|m| m.id == id) {
                m.pending = true;
            }
            cmd_tx
                .send(WorkerCmd::SetInput { id, input: target })
                .ok();
        }
        NetEvt::SwitchResult { id, ok, error } => {
            if let Some(m) = state.monitors.iter_mut().find(|m| m.id == id) {
                m.pending = false;
            }
            if !ok {
                state.status = error.unwrap_or_else(|| "switch failed".to_string());
            }
        }
        NetEvt::Status(s) => state.status = s,
    }
}

/// Update `existing` in place from a fresh worker snapshot: matched monitors
/// keep their `pending`/`owner` fields, unmatched incoming ones are appended,
/// and existing ones missing from `incoming` are marked unavailable. Never
/// rebuilds the vec wholesale so egui's widget state stays stable.
fn merge_snapshots(existing: &mut Vec<MonitorState>, incoming: Vec<MonitorSnapshot>) {
    for snap in &incoming {
        if let Some(m) = existing.iter_mut().find(|m| m.id == snap.id) {
            m.name = snap.name.clone();
            m.current_input = snap.current_input;
            m.valid_inputs = snap.valid_inputs.clone();
            m.available = snap.available;
        } else {
            existing.push(MonitorState {
                id: snap.id.clone(),
                name: snap.name.clone(),
                current_input: snap.current_input,
                valid_inputs: snap.valid_inputs.clone(),
                available: snap.available,
                owner: Owner::Me, // phase 2 is single-machine: every enumerated monitor is ours.
                pending: false,
            });
        }
    }
    for m in existing.iter_mut() {
        if !incoming.iter().any(|s| s.id == m.id) {
            m.available = false;
        }
    }
}

fn apply_set_result(state: &mut AppState, id: StableId, result: Result<InputSource, String>) {
    match result {
        Ok(input) => {
            if let Some(m) = state.monitors.iter_mut().find(|m| m.id == id) {
                m.pending = false;
                m.current_input = Some(input);
            }
        }
        Err(e) => {
            if let Some(m) = state.monitors.iter_mut().find(|m| m.id == id) {
                m.pending = false;
            }
            state.status = e;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(model: &str) -> StableId {
        crate::ddc::identity::stable_id(Some("acme"), Some(model), None, Some("serial"), None, 0)
    }

    #[test]
    fn merge_updates_existing_and_preserves_pending_and_appends_new() {
        let mut existing = vec![MonitorState {
            id: id("a"),
            name: "A".into(),
            current_input: Some(InputSource(0x11)),
            valid_inputs: vec![InputSource(0x11)],
            available: true,
            owner: Owner::Me,
            pending: true,
        }];
        let incoming = vec![
            MonitorSnapshot {
                id: id("a"),
                name: "A".into(),
                current_input: Some(InputSource(0x12)),
                valid_inputs: vec![InputSource(0x11), InputSource(0x12)],
                available: true,
            },
            MonitorSnapshot {
                id: id("b"),
                name: "B".into(),
                current_input: None,
                valid_inputs: vec![],
                available: true,
            },
        ];

        merge_snapshots(&mut existing, incoming);

        assert_eq!(existing.len(), 2);
        assert_eq!(existing[0].current_input, Some(InputSource(0x12)));
        assert_eq!(existing[0].owner, Owner::Me, "owner must be preserved across a merge");
        assert!(existing[0].pending, "pending flag must be preserved across a merge");
        assert_eq!(existing[1].id, id("b"));
        assert!(!existing[1].pending);
    }

    #[test]
    fn merge_marks_monitors_missing_from_the_snapshot_unavailable() {
        let mut existing = vec![MonitorState {
            id: id("a"),
            name: "A".into(),
            current_input: Some(InputSource(0x11)),
            valid_inputs: vec![InputSource(0x11)],
            available: true,
            owner: Owner::Unknown,
            pending: false,
        }];

        merge_snapshots(&mut existing, vec![]);

        assert!(!existing[0].available);
    }

    #[test]
    fn set_result_ok_clears_pending_and_updates_input() {
        let mut state = AppState {
            monitors: vec![MonitorState {
                id: id("a"),
                name: "A".into(),
                current_input: Some(InputSource(0x11)),
                valid_inputs: vec![InputSource(0x11), InputSource(0x12)],
                available: true,
                owner: Owner::Me,
                pending: true,
            }],
            ..Default::default()
        };

        apply_set_result(&mut state, id("a"), Ok(InputSource(0x12)));

        assert!(!state.monitors[0].pending);
        assert_eq!(state.monitors[0].current_input, Some(InputSource(0x12)));
    }

    #[test]
    fn set_result_err_clears_pending_and_sets_status() {
        let mut state = AppState {
            monitors: vec![MonitorState {
                id: id("a"),
                name: "A".into(),
                current_input: Some(InputSource(0x11)),
                valid_inputs: vec![InputSource(0x11)],
                available: true,
                owner: Owner::Me,
                pending: true,
            }],
            ..Default::default()
        };

        apply_set_result(&mut state, id("a"), Err("write failed".to_string()));

        assert!(!state.monitors[0].pending);
        assert_eq!(state.status, "write failed");
    }
}
