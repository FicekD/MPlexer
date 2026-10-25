mod config;
mod ddc;
mod logging;
mod model;
mod net;
mod ui;
mod worker;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ddc::{InputSource, Monitor};

#[derive(Parser)]
#[command(name = "mplexer")]
struct Cli {
    /// Run the debugging harness instead of the GUI.
    #[arg(long)]
    cli: bool,

    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Subcommand)]
enum CliCommand {
    /// Enumerate monitors and print their id, name, current input and valid inputs.
    List,
    /// Read and print the current input for one monitor.
    Get {
        /// Full StableId string, or `#index` from `list`.
        id: String,
    },
    /// Write the input source for one monitor. Requires --yes to actually write.
    Set {
        /// Full StableId string, or `#index` from `list`.
        id: String,
        /// VCP 0x60 input code, e.g. 0x11 or 11.
        hex_code: String,
        /// Actually perform the write. Without this, only prints what would happen.
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let log_buffer = logging::init();

    let cli = Cli::parse();

    if !cli.cli {
        return run_gui(log_buffer);
    }

    let Some(command) = cli.command else {
        anyhow::bail!("provide a subcommand: list | get | set");
    };

    match command {
        CliCommand::List => list(),
        CliCommand::Get { id } => get(&id),
        CliCommand::Set { id, hex_code, yes } => set(&id, &hex_code, yes),
    }
}

fn run_gui(log_buffer: logging::LogBuffer) -> Result<()> {
    let cfg = match config::Config::load() {
        Ok(cfg) => {
            if let Err(e) = cfg.save() {
                log::warn!("failed to write config: {e:#}");
            }
            cfg
        }
        Err(e) => {
            log::error!(
                "config is unreadable, running with defaults and leaving the file untouched: {e:#}"
            );
            config::Config::default()
        }
    };
    log::info!(
        "starting GUI as '{}' on comms port {}",
        cfg.machine_name,
        cfg.comms_port
    );

    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (evt_tx, evt_rx) = std::sync::mpsc::channel();
    let (net_evt_tx, net_evt_rx) = std::sync::mpsc::channel();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([900.0, 600.0])
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "MPlexer",
        options,
        Box::new(move |cc| {
            worker::spawn(cmd_rx, evt_tx, cc.egui_ctx.clone(), cfg.observed_inputs.clone());
            let net = net::spawn(cfg.clone(), net_evt_tx, cc.egui_ctx.clone());
            Ok(Box::new(ui::MplexerApp::new(
                cfg, cmd_tx, evt_rx, net, net_evt_rx, log_buffer,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))
}

fn list() -> Result<()> {
    let cfg = config::Config::load().unwrap_or_default();
    for (index, monitor) in ddc::enumerate().iter().enumerate() {
        let observed = cfg.observed_inputs.get(&monitor.id).map(Vec::as_slice).unwrap_or(&[]);
        let controllable = monitor.controllable || !observed.is_empty();
        println!(
            "#{}  id={}  name=\"{}\"  controllable={}  current={}  observed=[{}]",
            index,
            monitor.id.as_str(),
            monitor.name,
            controllable,
            format_input(monitor.current_input),
            observed.iter().map(|i| i.label()).collect::<Vec<_>>().join(", "),
        );
    }
    Ok(())
}

fn get(id: &str) -> Result<()> {
    let mut monitors = ddc::enumerate();
    let monitor = resolve_monitor(&mut monitors, id)
        .with_context(|| format!("no monitor matching '{id}'"))?;
    let input = monitor.read_input()?;
    println!(
        "id={}  name=\"{}\"  current={}",
        monitor.id.as_str(),
        monitor.name,
        format_input(Some(input)),
    );
    Ok(())
}

fn set(id: &str, hex_code: &str, yes: bool) -> Result<()> {
    let code = parse_hex_code(hex_code)?;
    let input = InputSource(code);

    let mut monitors = ddc::enumerate();
    let monitor = resolve_monitor(&mut monitors, id)
        .with_context(|| format!("no monitor matching '{id}'"))?;

    if !yes {
        println!(
            "WOULD write VCP 0x60 = {} on \"{}\" (id={}). This can blank the screen; re-run with --yes to actually switch.",
            input.label(),
            monitor.name,
            monitor.id.as_str(),
        );
        return Ok(());
    }

    println!(
        "WARNING: switching \"{}\" (id={}) to {} now.",
        monitor.name,
        monitor.id.as_str(),
        input.label(),
    );
    monitor.write_input(input)?;
    println!("Done.");
    Ok(())
}

fn resolve_monitor<'a>(monitors: &'a mut [Monitor], id: &str) -> Option<&'a mut Monitor> {
    if let Some(index) = id.strip_prefix('#') {
        let index: usize = index.parse().ok()?;
        return monitors.get_mut(index);
    }
    monitors.iter_mut().find(|m| m.id.as_str() == id)
}

fn parse_hex_code(s: &str) -> Result<u8> {
    let trimmed = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u8::from_str_radix(trimmed, 16).with_context(|| format!("invalid hex code '{s}'"))
}

fn format_input(input: Option<InputSource>) -> String {
    match input {
        Some(i) => i.label(),
        None => "unknown".to_string(),
    }
}
