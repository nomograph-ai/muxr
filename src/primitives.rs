//! Campaign primitives for muxr-managed repos.
//!
//! A campaign is a long-lived initiative. It lives in its own directory at
//! `campaigns/<campaign>/` containing two files:
//!
//! - `campaign.md` -- YAML frontmatter declaring `category:`, `paths:`,
//!   `synthesist_trees:`, and optional `sharded_from:`, plus a markdown body
//!   of conventions (what this is / how to behave).
//! - `log.md` -- YAML frontmatter declaring `entrypoint:`, plus a markdown
//!   body that is an append-only log.
//!
//! Muxr composes the repo's HARNESS prompt + campaign body + log body into
//! the runtime's system prompt at launch. Campaign `paths:` are passed as
//! `--add-dir`, so the tool knows the full work surface. Sessions are named
//! `<repo>/<campaign>` -- two levels, one session per campaign.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Campaign frontmatter (`campaigns/<campaign>/campaign.md`).
#[derive(Debug, Default, Deserialize)]
pub struct Campaign {
    /// Classification slug (was the middle segment in the old 3-level name).
    /// Parsed now; surfaced by the chooser (W5) and inherited by `shard` (W9).
    #[serde(default)]
    #[allow(dead_code)]
    pub category: String,
    #[serde(default)]
    pub synthesist_trees: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    /// Lineage: the parent campaign this one was sharded out of, if any.
    /// Parsed now; consumed by the chooser grouping (W5) and `shard` (W9).
    #[serde(default)]
    #[allow(dead_code)]
    pub sharded_from: Option<String>,
}

/// Log frontmatter (`campaigns/<campaign>/log.md`).
#[derive(Debug, Default, Deserialize)]
pub struct Log {
    #[serde(default)]
    pub entrypoint: String,
}

/// Split a markdown file into (YAML frontmatter, markdown body).
///
/// Expects the file to start with `---`, a YAML block, then a line that
/// is just `---`. Everything after is the body.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let trimmed = content.trim_start_matches('\u{feff}');
    let after_opening = trimmed.strip_prefix("---")?;
    let after_opening = after_opening.trim_start_matches('\r').strip_prefix('\n')?;
    let end_marker = after_opening.find("\n---")?;
    let fm = &after_opening[..end_marker];
    let rest = &after_opening[end_marker + 4..];
    let body = rest
        .strip_prefix("\r\n")
        .unwrap_or_else(|| rest.strip_prefix('\n').unwrap_or(rest));
    Some((fm, body))
}

/// `<repo-dir>/campaigns/<campaign>/`.
pub fn campaign_dir(repo_dir: &Path, campaign: &str) -> PathBuf {
    repo_dir.join("campaigns").join(campaign)
}

/// `<repo-dir>/campaigns/<campaign>/campaign.md`.
pub fn campaign_md_path(repo_dir: &Path, campaign: &str) -> PathBuf {
    campaign_dir(repo_dir, campaign).join("campaign.md")
}

/// `<repo-dir>/campaigns/<campaign>/log.md`.
pub fn log_md_path(repo_dir: &Path, campaign: &str) -> PathBuf {
    campaign_dir(repo_dir, campaign).join("log.md")
}

pub fn load_campaign(path: &Path) -> Result<(Campaign, String)> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read campaign file: {}", path.display()))?;
    let (fm, body) = split_frontmatter(&content)
        .with_context(|| format!("No YAML frontmatter in {}", path.display()))?;
    let campaign: Campaign = serde_yaml_ng::from_str(fm)
        .with_context(|| format!("Failed to parse campaign frontmatter: {}", path.display()))?;
    Ok((campaign, body.to_string()))
}

pub fn load_log(path: &Path) -> Result<(Log, String)> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read log file: {}", path.display()))?;
    let (fm, body) = split_frontmatter(&content)
        .with_context(|| format!("No YAML frontmatter in {}", path.display()))?;
    let log: Log = serde_yaml_ng::from_str(fm)
        .with_context(|| format!("Failed to parse log frontmatter: {}", path.display()))?;
    Ok((log, body.to_string()))
}

/// Resolve `<repo-dir>/campaigns/<campaign>/campaign.md`, erroring if
/// the campaign does not exist.
pub fn campaign_file(repo_dir: &Path, campaign: &str) -> Result<PathBuf> {
    let path = campaign_md_path(repo_dir, campaign);
    if !path.is_file() {
        anyhow::bail!("Campaign '{campaign}' not found at {}.", path.display());
    }
    Ok(path)
}

/// Reserved campaign slug for the repo switchboard. One per repo.
/// Launched by `muxr <repo>` with no campaign arg, as `<repo>/switchboard`.
pub const SWITCHBOARD: &str = "switchboard";

/// Validate a campaign slug. Campaigns name directories and tmux sessions, so
/// they must be filesystem- and tmux-safe.
///
/// Rules: kebab-case (lowercase letters, digits, hyphens), 1-64 chars,
/// no leading/trailing/consecutive hyphens.
pub fn validate_topic(campaign: &str) -> Result<()> {
    if campaign.is_empty() {
        anyhow::bail!(
            "Campaign required: muxr <repo> <campaign>.\n\
             Campaign is kebab-case and names the initiative (e.g. 'cicd-stub-fix')."
        );
    }
    if campaign.len() > 64 {
        anyhow::bail!(
            "Campaign too long: {} chars (max 64). Pick something shorter.",
            campaign.len()
        );
    }
    for c in campaign.chars() {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            anyhow::bail!(
                "Campaign must be kebab-case (lowercase letters, digits, hyphens).\n\
                 Invalid char: '{c}'. Try kebab-case like 'cicd-stub-fix'."
            );
        }
    }
    // Reject empty segments: catches leading hyphen, trailing hyphen, and
    // consecutive hyphens in one rule.
    if campaign.split('-').any(|s| s.is_empty()) {
        anyhow::bail!(
            "Campaign must not have leading, trailing, or consecutive hyphens.\n\
             Got '{campaign}'. Try kebab-case like 'cicd-stub-fix'."
        );
    }
    Ok(())
}

/// Scaffold the switchboard campaign for a repo if it doesn't exist.
///
/// The switchboard is the per-repo orchestrator AI. It lives at
/// `campaigns/switchboard/` and gets a specific persona + bootstrap
/// log entrypoint, distinct from regular campaign scaffolding.
pub fn scaffold_switchboard(repo_dir: &Path) -> Result<PathBuf> {
    let dir = campaign_dir(repo_dir, SWITCHBOARD);
    fs::create_dir_all(&dir)?;

    let campaign_md = campaign_md_path(repo_dir, SWITCHBOARD);
    if !campaign_md.is_file() {
        let repo_name = repo_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("repo");
        let content = format!(
            "---\ncategory: switchboard\nsynthesist_trees: []\npaths: []\n---\n\n\
             # {repo_name} switchboard\n\n\
             ## What this is\n\
             The per-repo orchestrator. One AI session whose job is to \
             help the human spawn, triage, archive, and navigate campaigns \
             in this repo without memorizing muxr commands. Scope is \
             this repo only -- cross-repo work happens at the \
             control-plane shell.\n\n\
             ## How to behave\n\
             - Classify intent fast. Propose, don't interrogate.\n\
             - \"I want to work on X\" -> glob campaigns/*/ to see what \
             exists; if X is there, run `muxr {repo_name} X` to launch \
             it in a new tmux session (via Bash). If not, propose paths \
             from the add_dirs and run the scaffold launch.\n\
             - \"What's going on\" -> `synthesist status`, `muxr ls --active`, \
             summarize.\n\
             - Delegate actual work to campaign sessions. This pane is a \
             dispatcher, not a work pane. Keep conversations short.\n\
             - /serialize rarely here -- the switchboard isn't a work \
             session. Update its log only when the repo itself changed \
             (new campaign added, old one archived, structural shift).\n"
        );
        fs::write(&campaign_md, content)?;
    }

    // Seed the switchboard log if missing. The switchboard is one
    // accumulating log per repo.
    let log_md = log_md_path(repo_dir, SWITCHBOARD);
    if !log_md.exists() {
        let content = "---\nentrypoint: \"Switchboard ready. First-glance: run `synthesist status` and ls campaigns/ so you know what's live. Then wait for the human's intent.\"\n---\n\n\
             # Switchboard\n\n\
             ## Log\n\
             Switchboard scaffolded.\n"
            .to_string();
        fs::write(&log_md, content)?;
    }

    Ok(campaign_md)
}

/// Scaffold a stub campaign directory (`campaign.md` + `log.md`) that tells
/// the tool to onboard the human conversationally.
///
/// Muxr does NOT prompt for paths/tree/description at the terminal.
/// Instead it creates empty stubs and seeds the log's entrypoint with an
/// instruction for the tool to ask the human what the campaign is about
/// and populate campaign.md via Edit. This keeps the launch command
/// single-keystroke and moves the onboarding into a natural LLM
/// conversation where typos, ambiguity, and defaults are cheap.
pub fn scaffold_campaign_stub(repo_dir: &Path, campaign: &str) -> Result<PathBuf> {
    let dir = campaign_dir(repo_dir, campaign);
    fs::create_dir_all(&dir)?;

    let campaign_content = format!(
        "---\ncategory: \"\"\nsynthesist_trees: []\npaths: []\n---\n\n\
         # {campaign}\n\n\
         ## What this is\n\
         (pending -- the tool will prompt the human on first launch)\n\n\
         ## How to behave\n\
         (pending)\n"
    );
    let campaign_md = campaign_md_path(repo_dir, campaign);
    fs::write(&campaign_md, campaign_content)?;

    // Seed the log with a bootstrap entrypoint so the tool knows to run the
    // onboarding conversation on first response.
    let log_md = log_md_path(repo_dir, campaign);
    if !log_md.exists() {
        let entrypoint = format!(
            "Bootstrap campaign '{campaign}'. campaign.md is a stub. \
             First action: discover, don't interrogate. Search ~/gitlab.com, \
             ~/github.com, and synthesist trees for repos/dirs/items that \
             match the slug. Propose candidate paths and a tree mapping to \
             the human for confirmation or correction. Keep it to one \
             confirm-and-go exchange. Write the confirmed values into \
             campaign.md via Edit, then proceed with whatever work the \
             human wants."
        );
        let log_content = format!(
            "---\nentrypoint: \"{entrypoint}\"\n---\n\n\
             # {campaign}\n\n\
             ## Log\n\
             Freshly scaffolded campaign. Awaiting onboarding conversation.\n"
        );
        fs::write(&log_md, log_content)?;
    }

    eprintln!();
    eprintln!("Scaffolded stub campaign: {}", campaign_md.display());
    eprintln!("The tool will prompt you to fill it out on launch.");
    eprintln!();

    Ok(campaign_md)
}

/// Find or scaffold the log file for the given campaign.
/// If `campaigns/<campaign>/log.md` exists, returns it. Otherwise scaffolds
/// the campaign dir + a minimal log.md and returns the new path.
pub fn resolve_or_scaffold_session(repo_dir: &Path, campaign: &str) -> Result<PathBuf> {
    let log_md = log_md_path(repo_dir, campaign);
    if log_md.is_file() {
        return Ok(log_md);
    }

    let dir = campaign_dir(repo_dir, campaign);
    fs::create_dir_all(&dir)?;
    let content = format!("---\nentrypoint: \"\"\n---\n\n# {campaign}\n\n## Log\n\n");
    fs::write(&log_md, content)?;
    Ok(log_md)
}

/// Compose the system prompt addition from campaign + log bodies.
pub fn compose_prompt(campaign: &str, campaign_body: &str, log_body: &str) -> String {
    format!(
        "# Campaign: {campaign}\n\n{}\n\n---\n\n# Log\n\n{}",
        campaign_body.trim(),
        log_body.trim()
    )
}

/// Expand `~` in a path string to the user's home directory.
pub fn expand_home(path: &str) -> String {
    shellexpand::tilde(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_basic() {
        let content = "---\nfoo: bar\n---\nbody text\n";
        let (fm, body) = split_frontmatter(content).unwrap();
        assert_eq!(fm, "foo: bar");
        assert_eq!(body, "body text\n");
    }

    #[test]
    fn split_frontmatter_multiline() {
        let content = "---\nfoo: bar\nbaz: qux\n---\n\n# Title\n\nMore body.\n";
        let (fm, body) = split_frontmatter(content).unwrap();
        assert_eq!(fm, "foo: bar\nbaz: qux");
        assert!(body.starts_with("\n# Title") || body.starts_with("# Title"));
    }

    #[test]
    fn split_frontmatter_missing_returns_none() {
        let content = "no frontmatter here\n";
        assert!(split_frontmatter(content).is_none());
    }

    #[test]
    fn parse_campaign_frontmatter() {
        let fm =
            "category: harness\nsynthesist_trees:\n  - harness\npaths:\n  - ~/foo\n  - ~/bar\n";
        let c: Campaign = serde_yaml_ng::from_str(fm).unwrap();
        assert_eq!(c.category, "harness");
        assert_eq!(c.synthesist_trees, vec!["harness"]);
        assert_eq!(c.paths, vec!["~/foo".to_string(), "~/bar".to_string()]);
        assert!(c.sharded_from.is_none());
    }

    #[test]
    fn parse_campaign_with_sharded_from() {
        let fm = "category: customer\nsharded_from: ncbi\npaths: []\n";
        let c: Campaign = serde_yaml_ng::from_str(fm).unwrap();
        assert_eq!(c.sharded_from.as_deref(), Some("ncbi"));
    }

    #[test]
    fn parse_log_frontmatter() {
        let fm = "entrypoint: do the thing\n";
        let l: Log = serde_yaml_ng::from_str(fm).unwrap();
        assert_eq!(l.entrypoint, "do the thing");
    }

    #[test]
    fn parse_campaign_defaults_to_empty_lists() {
        let fm = "";
        let c: Campaign = serde_yaml_ng::from_str(fm).unwrap_or_default();
        assert!(c.synthesist_trees.is_empty());
        assert!(c.paths.is_empty());
        assert!(c.category.is_empty());
    }

    #[test]
    fn compose_prompt_includes_both_bodies() {
        let out = compose_prompt("gkg", "## What\ngkg stuff", "## Log\nentry");
        assert!(out.contains("Campaign: gkg"));
        assert!(out.contains("gkg stuff"));
        assert!(out.contains("# Log"));
        assert!(out.contains("entry"));
    }

    #[test]
    fn resolve_or_scaffold_creates_log_in_campaign_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        let dir = campaign_dir(repo_dir, "gkg");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("campaign.md"),
            "---\ncategory: \"\"\npaths: []\n---\n\n# gkg\n",
        )
        .unwrap();

        let path = resolve_or_scaffold_session(repo_dir, "gkg").unwrap();
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "log.md");
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("# gkg"));
    }

    #[test]
    fn resolve_or_scaffold_attaches_to_existing_log() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        let dir = campaign_dir(repo_dir, "gkg");
        fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("log.md");
        fs::write(&existing, "---\nentrypoint: x\n---\n\n# gkg\n").unwrap();

        let path = resolve_or_scaffold_session(repo_dir, "gkg").unwrap();
        assert_eq!(path, existing);
    }

    #[test]
    fn validate_topic_accepts_kebab_case() {
        assert!(validate_topic("cicd-stub-fix").is_ok());
        assert!(validate_topic("a").is_ok());
        assert!(validate_topic("topic-flag").is_ok());
        assert!(validate_topic("v2-rewrite").is_ok());
        assert!(validate_topic("0-warmup").is_ok());
    }

    #[test]
    fn validate_topic_rejects_empty() {
        let err = validate_topic("").unwrap_err().to_string();
        assert!(err.contains("Campaign required"));
    }

    #[test]
    fn validate_topic_rejects_uppercase() {
        assert!(validate_topic("Topic").is_err());
        assert!(validate_topic("MyTopic").is_err());
    }

    #[test]
    fn validate_topic_rejects_slash_and_space() {
        assert!(validate_topic("foo/bar").is_err());
        assert!(validate_topic("foo bar").is_err());
        assert!(validate_topic("foo.bar").is_err());
    }

    #[test]
    fn validate_topic_rejects_leading_hyphen_or_underscore() {
        assert!(validate_topic("-foo").is_err());
        assert!(validate_topic("_foo").is_err());
    }

    #[test]
    fn validate_topic_rejects_trailing_hyphen() {
        assert!(validate_topic("foo-").is_err());
    }

    #[test]
    fn validate_topic_rejects_consecutive_hyphens() {
        assert!(validate_topic("foo--bar").is_err());
        assert!(validate_topic("a---b").is_err());
    }

    #[test]
    fn validate_topic_rejects_lone_hyphen() {
        assert!(validate_topic("-").is_err());
    }

    #[test]
    fn validate_topic_rejects_too_long() {
        let long = "a".repeat(65);
        assert!(validate_topic(&long).is_err());
        let max = "a".repeat(64);
        assert!(validate_topic(&max).is_ok());
    }

    #[test]
    fn scaffold_switchboard_creates_campaign_and_log() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        scaffold_switchboard(repo_dir).unwrap();

        let log_md = log_md_path(repo_dir, SWITCHBOARD);
        assert!(log_md.is_file(), "switchboard log.md should exist");

        let contents = fs::read_to_string(&log_md).unwrap();
        assert!(contents.contains("# Switchboard"));

        let campaign_md = campaign_md_path(repo_dir, SWITCHBOARD);
        assert!(campaign_md.is_file());
        assert!(
            fs::read_to_string(&campaign_md)
                .unwrap()
                .contains("switchboard")
        );
    }

    #[test]
    fn scaffold_campaign_stub_writes_campaign_and_log() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        scaffold_campaign_stub(repo_dir, "retrieval-precision").unwrap();

        let campaign_md = campaign_md_path(repo_dir, "retrieval-precision");
        assert!(campaign_md.is_file());

        let log_md = log_md_path(repo_dir, "retrieval-precision");
        assert!(log_md.is_file(), "log.md should exist");
        let body = fs::read_to_string(&log_md).unwrap();
        assert!(body.contains("# retrieval-precision"));
        assert!(body.contains("Bootstrap campaign 'retrieval-precision'"));
    }

    #[test]
    fn scaffold_switchboard_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        scaffold_switchboard(repo_dir).unwrap();
        let log_md = log_md_path(repo_dir, SWITCHBOARD);
        fs::write(&log_md, "custom content").unwrap();

        scaffold_switchboard(repo_dir).unwrap();
        assert_eq!(fs::read_to_string(&log_md).unwrap(), "custom content");
    }
}
