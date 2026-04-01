use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_tool")]
    pub default_tool: String,
    pub verticals: HashMap<String, Vertical>,
    #[serde(default)]
    pub remotes: HashMap<String, Remote>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Vertical {
    pub dir: String,
    pub color: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Remote {
    pub project: String,
    pub zone: String,
    pub user: String,
    pub color: String,
    #[serde(default = "default_connect")]
    pub connect: String,
    #[serde(default)]
    pub instance_prefix: Option<String>,
}

fn default_tool() -> String {
    "claude".to_string()
}

fn default_connect() -> String {
    "mosh".to_string()
}

impl Remote {
    /// Derive a GCE instance name from the context.
    /// Replaces `/` with `-` so nested contexts produce valid instance names.
    pub fn instance_name(&self, context: &str) -> String {
        let slug = context.replace('/', "-");
        match &self.instance_prefix {
            Some(prefix) => format!("{prefix}{slug}"),
            None => slug,
        }
    }
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

        // Validate no name collisions between verticals and remotes
        for name in config.remotes.keys() {
            if config.verticals.contains_key(name) {
                anyhow::bail!(
                    "Name collision: '{name}' is defined as both a vertical and a remote"
                );
            }
        }

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

    /// All known names (verticals + remotes) for validation and completions.
    pub fn all_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .verticals
            .keys()
            .chain(self.remotes.keys())
            .map(|s| s.as_str())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn is_remote(&self, name: &str) -> bool {
        self.remotes.contains_key(name)
    }

    pub fn remote(&self, name: &str) -> Option<&Remote> {
        self.remotes.get(name)
    }

    pub fn color_for(&self, name: &str) -> &str {
        self.verticals
            .get(name)
            .map(|v| v.color.as_str())
            .or_else(|| self.remotes.get(name).map(|r| r.color.as_str()))
            .unwrap_or("#8a7f83")
    }
}
