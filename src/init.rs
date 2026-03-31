use crate::config::Config;
use anyhow::{Context, Result};

/// Create a default config file.
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

    let default_config = r##"# muxr configuration
# Verticals define your project estates.
# Each vertical maps to a directory and a status bar color.

default_tool = "claude"

# Add your verticals here. Examples:
#
# [verticals.work]
# dir = "~/projects/work"
# color = "#7aa2f7"
#
# [verticals.personal]
# dir = "~/projects/personal"
# color = "#9ece6a"
#
# [verticals.oss]
# dir = "~/projects/oss"
# color = "#73daca"
"##;

    std::fs::write(&path, default_config)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    eprintln!("Created config at {}", path.display());
    eprintln!("Edit it to add your verticals, then run `muxr <vertical>`.");

    Ok(())
}
