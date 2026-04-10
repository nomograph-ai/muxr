use crate::config::Config;
use anyhow::{Context, Result};

/// Create a default config file.
/// Template is derived from Config::default_template() so the structure
/// stays in sync with the Config struct.
pub fn init() -> Result<()> {
    let path = Config::path()?;

    if path.exists() {
        eprintln!("Config already exists at {}", path.display());
        eprintln!("Edit it directly or delete it to re-initialize.");
        return Ok(());
    }

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    std::fs::write(&path, Config::default_template())
        .with_context(|| format!("Failed to write {}", path.display()))?;

    eprintln!("Created config at {}", path.display());
    eprintln!("Edit it to add your verticals, then run `muxr <vertical>`.");

    Ok(())
}
