use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::ddc::{InputSource, StableId};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub machine_name: String,
    #[serde(default)]
    pub input_labels: HashMap<String, String>,
    #[serde(default = "Uuid::new_v4")]
    pub machine_id: Uuid,
    #[serde(default = "default_comms_port")]
    pub comms_port: u16,
    #[serde(default)]
    pub observed_inputs: HashMap<StableId, Vec<InputSource>>,
    #[serde(default = "default_dark")]
    pub dark_theme: bool,
    #[serde(default = "default_ui_scale")]
    pub ui_scale: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            machine_name: hostname(),
            input_labels: HashMap::new(),
            machine_id: Uuid::new_v4(),
            comms_port: default_comms_port(),
            observed_inputs: HashMap::new(),
            dark_theme: default_dark(),
            ui_scale: default_ui_scale(),
        }
    }
}

fn default_comms_port() -> u16 {
    crate::net::TCP_PORT
}

fn default_dark() -> bool {
    true
}

fn default_ui_scale() -> u32 {
    100
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "MPlexer")
            .context("could not determine a config directory for this platform")?;
        Ok(dirs.config_dir().join("config.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))
    }

    pub fn persist_with(f: impl FnOnce(&mut Config)) -> Result<()> {
        let mut cfg = Self::load().unwrap_or_else(|_| Self::default());
        f(&mut cfg);
        cfg.save()
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "mplexer-host".to_string())
}
