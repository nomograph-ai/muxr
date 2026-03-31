use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_tool")]
    pub default_tool: String,
    pub verticals: HashMap<String, Vertical>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Vertical {
    pub dir: String,
    pub color: String,
}

fn default_tool() -> String {
    "claude".to_string()
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            anyhow::bail!(
                "No config found at {}\nRun `muxr init` to create one.",
                path.display()
            );
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        Ok(config)
    }

    pub fn path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        let config_dir = home.join(".config").join("muxr");
        Ok(config_dir.join("config.toml"))
    }

    pub fn state_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        let config_dir = home.join(".config").join("muxr");
        Ok(config_dir.join("state.json"))
    }

    pub fn resolve_dir(&self, vertical: &str) -> Result<PathBuf> {
        let v = self
            .verticals
            .get(vertical)
            .with_context(|| format!("Unknown vertical: {vertical}"))?;
        let expanded = shellexpand::tilde(&v.dir);
        Ok(PathBuf::from(expanded.as_ref()))
    }

    pub fn vertical_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.verticals.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    pub fn color_for(&self, vertical: &str) -> &str {
        self.verticals
            .get(vertical)
            .map(|v| v.color.as_str())
            .unwrap_or("#8a7f83")
    }
}
