use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub machine_name: String,
    pub input_labels: HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            machine_name: hostname(),
            input_labels: HashMap::new(),
        }
    }
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
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "mplexer-host".to_string())
}
