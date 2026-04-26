//! Campaign/session primitives for muxr-managed harnesses.
//!
//! A campaign is a long-lived body of work. Lives at
//! `campaigns/<slug>/campaign.md` with YAML frontmatter declaring
//! `synthesist_trees:` and `paths:`, and a markdown body of conventions.
//!
//! A session is an ephemeral episode. Lives at
//! `campaigns/<slug>/sessions/<date>[-<suffix>].md` with YAML
//! frontmatter declaring `campaign:` + `entrypoint:`, and a markdown
//! body that is an append-only log.
//!
//! Muxr composes `HARNESS.md` + campaign body + session body into the
//! runtime's system prompt at launch. Campaign `paths:` are passed as
//! `--add-dir`, so Claude knows the full work surface.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Campaign frontmatter (`campaigns/<slug>/campaign.md`).
#[derive(Debug, Default, Deserialize)]
pub struct Campaign {
    #[serde(default)]
    pub synthesist_trees: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

/// Session frontmatter (`campaigns/<slug>/sessions/<date>[-<suffix>].md`).
#[derive(Debug, Deserialize)]
pub struct Session {
    pub campaign: String,
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

pub fn load_campaign(path: &Path) -> Result<(Campaign, String)> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read campaign file: {}", path.display()))?;
    let (fm, body) = split_frontmatter(&content)
        .with_context(|| format!("No YAML frontmatter in {}", path.display()))?;
    let campaign: Campaign = serde_yaml_ng::from_str(fm)
        .with_context(|| format!("Failed to parse campaign frontmatter: {}", path.display()))?;
    Ok((campaign, body.to_string()))
}

pub fn load_session(path: &Path) -> Result<(Session, String)> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read session file: {}", path.display()))?;
    let (fm, body) = split_frontmatter(&content)
        .with_context(|| format!("No YAML frontmatter in {}", path.display()))?;
    let session: Session = serde_yaml_ng::from_str(fm)
        .with_context(|| format!("Failed to parse session frontmatter: {}", path.display()))?;
    Ok((session, body.to_string()))
}

/// Resolve `<harness-dir>/campaigns/<campaign>/campaign.md`, erroring if
/// the campaign does not exist.
pub fn campaign_file(harness_dir: &Path, campaign: &str) -> Result<PathBuf> {
    let path = harness_dir
        .join("campaigns")
        .join(campaign)
        .join("campaign.md");
    if !path.is_file() {
        anyhow::bail!("Campaign '{campaign}' not found at {}.", path.display());
    }
    Ok(path)
}

/// Reserved campaign slug for the harness switchboard. One per harness.
/// Launched by `muxr <harness>` with no campaign arg.
pub const SWITCHBOARD: &str = "_switchboard";

/// Scaffold the switchboard campaign for a harness if it doesn't exist.
///
/// The switchboard is the per-harness orchestrator AI. It lives at
/// `campaigns/_switchboard/` and gets a specific persona + bootstrap
/// entrypoint, distinct from regular campaign scaffolding.
pub fn scaffold_switchboard(harness_dir: &Path) -> Result<PathBuf> {
    let campaign_dir = harness_dir.join("campaigns").join(SWITCHBOARD);
    let sessions_dir = campaign_dir.join("sessions");
    fs::create_dir_all(&sessions_dir)?;

    let campaign_md = campaign_dir.join("campaign.md");
    if !campaign_md.is_file() {
        let harness_name = harness_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("harness");
        let content = format!(
            "---\nsynthesist_trees: []\npaths: []\n---\n\n\
             # {harness_name} switchboard\n\n\
             ## What this is\n\
             The per-harness orchestrator. One AI session whose job is to \
             help the human spawn, triage, archive, and navigate campaigns \
             in this harness without memorizing muxr commands. Scope is \
             this harness only -- cross-harness work happens at the \
             control-plane shell.\n\n\
             ## How to behave\n\
             - Classify intent fast. Propose, don't interrogate.\n\
             - \"I want to work on X\" -> glob campaigns/*/ to see what \
             exists; if X is there, run `muxr {harness_name} X` to launch \
             it in a new tmux session (via Bash). If not, propose paths \
             from the add_dirs and run the scaffold launch.\n\
             - \"What's going on\" -> `synthesist status`, `muxr ls --active`, \
             summarize.\n\
             - Delegate actual work to campaign sessions. This pane is a \
             dispatcher, not a work pane. Keep conversations short.\n\
             - /serialize rarely here -- the switchboard isn't a work \
             session. Update its log only when the harness itself changed \
             (new campaign added, old one archived, structural shift).\n"
        );
        fs::write(&campaign_md, content)?;
    }

    // Seed today's session if missing
    let today = today();
    let session_path = sessions_dir.join(format!("{today}.md"));
    if !session_path.exists() {
        let content = format!(
            "---\ncampaign: {SWITCHBOARD}\nentrypoint: \"Switchboard ready. First-glance: run `synthesist status` and ls campaigns/ so you know what's live. Then wait for the human's intent.\"\n---\n\n\
             # Switchboard {today}\n\n\
             ## {today}\n\
             Switchboard session opened.\n"
        );
        fs::write(&session_path, content)?;
    }

    Ok(campaign_md)
}

/// Scaffold a stub campaign + a bootstrap session file that tells Claude
/// to onboard the human conversationally.
///
/// Muxr does NOT prompt for paths/tree/description at the terminal.
/// Instead it creates empty stubs and seeds the session's entrypoint
/// with an instruction for Claude to ask the human what the campaign
/// is about and populate campaign.md via Edit. This keeps the launch
/// command single-keystroke and moves the onboarding into a natural
/// LLM conversation where typos, ambiguity, and defaults are cheap.
pub fn scaffold_campaign_stub(harness_dir: &Path, campaign: &str) -> Result<PathBuf> {
    let campaign_dir = harness_dir.join("campaigns").join(campaign);
    let sessions_dir = campaign_dir.join("sessions");
    fs::create_dir_all(&sessions_dir)?;

    let campaign_content = format!(
        "---\nsynthesist_trees: []\npaths: []\n---\n\n\
         # {campaign}\n\n\
         ## What this is\n\
         (pending -- Claude will prompt the human on first launch)\n\n\
         ## How to behave\n\
         (pending)\n"
    );
    let campaign_md = campaign_dir.join("campaign.md");
    fs::write(&campaign_md, campaign_content)?;

    // Seed today's session file with a bootstrap entrypoint so Claude
    // knows to run the onboarding conversation on first response.
    let today = today();
    let session_path = sessions_dir.join(format!("{today}.md"));
    if !session_path.exists() {
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
        let session_content = format!(
            "---\ncampaign: {campaign}\nentrypoint: \"{entrypoint}\"\n---\n\n\
             # Session {today}\n\n\
             ## {today}\n\
             Freshly scaffolded campaign. Awaiting onboarding conversation.\n"
        );
        fs::write(&session_path, session_content)?;
    }

    eprintln!();
    eprintln!("Scaffolded stub campaign: {}", campaign_md.display());
    eprintln!("Claude will prompt you to fill it out on launch.");
    eprintln!();

    Ok(campaign_md)
}

/// Find or scaffold a session file for the given campaign and date.
/// If a file at `campaigns/<campaign>/sessions/<date>.md` exists, returns
/// it. Otherwise scaffolds from `campaigns/TEMPLATE/sessions/TEMPLATE.md`
/// (or a built-in fallback) and returns the new path.
pub fn resolve_or_scaffold_session(
    harness_dir: &Path,
    campaign: &str,
    date: &str,
) -> Result<PathBuf> {
    let campaign_dir = harness_dir.join("campaigns").join(campaign);
    let sessions_dir = campaign_dir.join("sessions");

    // If a same-date file already exists, prefer the plain one; if only
    // suffixed variants exist (e.g. 2026-04-24-cicd.md), return the first
    // one found so muxr attaches to ongoing work.
    let plain = sessions_dir.join(format!("{date}.md"));
    if plain.is_file() {
        return Ok(plain);
    }
    if sessions_dir.is_dir()
        && let Some(suffixed) = first_matching_session(&sessions_dir, date)?
    {
        return Ok(suffixed);
    }

    // Not found: scaffold at <date>.md
    fs::create_dir_all(&sessions_dir)?;
    let template_path = harness_dir
        .join("campaigns")
        .join("TEMPLATE")
        .join("sessions")
        .join("TEMPLATE.md");
    let content = if template_path.is_file() {
        let tpl = fs::read_to_string(&template_path)?;
        tpl.replace("<slug>", campaign)
            .replace("<date>[-<suffix>]", date)
    } else {
        format!(
            "---\ncampaign: {campaign}\nentrypoint: \"\"\n---\n\n# Session {date}\n\n## {date}\n\n"
        )
    };
    fs::write(&plain, content)?;
    Ok(plain)
}

/// Find the first `<date>[-<suffix>].md` file in `sessions_dir` whose
/// basename begins with `<date>`. Skips files in `archive/`.
fn first_matching_session(sessions_dir: &Path, date: &str) -> Result<Option<PathBuf>> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(sessions_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.ends_with(".md") {
            continue;
        }
        let exact = format!("{date}.md");
        let with_suffix = format!("{date}-");
        if name_str == exact || name_str.starts_with(&with_suffix) {
            candidates.push(entry.path());
        }
    }
    candidates.sort();
    Ok(candidates.into_iter().next())
}

/// Compose the system prompt addition from campaign + session bodies.
pub fn compose_prompt(campaign: &str, campaign_body: &str, session_body: &str) -> String {
    format!(
        "# Campaign: {campaign}\n\n{}\n\n---\n\n# Session\n\n{}",
        campaign_body.trim(),
        session_body.trim()
    )
}

/// Expand `~` in a path string to the user's home directory.
pub fn expand_home(path: &str) -> String {
    shellexpand::tilde(path).to_string()
}

/// Today's date as `YYYY-MM-DD`.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
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
        let fm = "synthesist_trees:\n  - harness\npaths:\n  - ~/foo\n  - ~/bar\n";
        let c: Campaign = serde_yaml_ng::from_str(fm).unwrap();
        assert_eq!(c.synthesist_trees, vec!["harness"]);
        assert_eq!(c.paths, vec!["~/foo".to_string(), "~/bar".to_string()]);
    }

    #[test]
    fn parse_session_frontmatter() {
        let fm = "campaign: harness\nentrypoint: do the thing\n";
        let s: Session = serde_yaml_ng::from_str(fm).unwrap();
        assert_eq!(s.campaign, "harness");
        assert_eq!(s.entrypoint, "do the thing");
    }

    #[test]
    fn parse_campaign_defaults_to_empty_lists() {
        let fm = "";
        let c: Campaign = serde_yaml_ng::from_str(fm).unwrap_or_default();
        assert!(c.synthesist_trees.is_empty());
        assert!(c.paths.is_empty());
    }

    #[test]
    fn compose_prompt_includes_both_bodies() {
        let out = compose_prompt("gkg", "## What\ngkg stuff", "## Log\nentry");
        assert!(out.contains("Campaign: gkg"));
        assert!(out.contains("gkg stuff"));
        assert!(out.contains("# Session"));
        assert!(out.contains("entry"));
    }

    #[test]
    fn today_has_iso_shape() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.chars().filter(|c| *c == '-').count() == 2);
    }

    #[test]
    fn resolve_or_scaffold_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let harness_dir = tmp.path();
        let campaign_dir = harness_dir.join("campaigns").join("gkg");
        fs::create_dir_all(&campaign_dir).unwrap();
        fs::write(
            campaign_dir.join("campaign.md"),
            "---\npaths: []\n---\n\n# gkg\n",
        )
        .unwrap();

        let path = resolve_or_scaffold_session(harness_dir, "gkg", "2026-04-24").unwrap();
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "2026-04-24.md");
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("campaign: gkg"));
    }

    #[test]
    fn resolve_or_scaffold_prefers_suffixed_same_day() {
        let tmp = tempfile::tempdir().unwrap();
        let harness_dir = tmp.path();
        let sessions_dir = harness_dir.join("campaigns").join("gkg").join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::write(
            sessions_dir.join("2026-04-24-cicd.md"),
            "---\ncampaign: gkg\nentrypoint: x\n---\n\n",
        )
        .unwrap();

        let path = resolve_or_scaffold_session(harness_dir, "gkg", "2026-04-24").unwrap();
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "2026-04-24-cicd.md"
        );
    }
}
