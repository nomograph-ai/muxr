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

/// Singleton topic used for the switchboard session file. The switchboard
/// is one accumulating log per harness, not per-topic.
pub const SWITCHBOARD_TOPIC: &str = "switchboard";

/// Validate a topic slug. Topics name session files and tmux sessions, so
/// they must be filesystem- and tmux-safe.
///
/// Rules: kebab-case (lowercase letters, digits, hyphens), 1-64 chars,
/// no leading/trailing/consecutive hyphens.
pub fn validate_topic(topic: &str) -> Result<()> {
    if topic.is_empty() {
        anyhow::bail!(
            "Topic required: muxr <harness> <campaign> <topic>.\n\
             Topic is kebab-case and describes the work (e.g. 'cicd-stub-fix')."
        );
    }
    if topic.len() > 64 {
        anyhow::bail!(
            "Topic too long: {} chars (max 64). Pick something shorter.",
            topic.len()
        );
    }
    for c in topic.chars() {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            anyhow::bail!(
                "Topic must be kebab-case (lowercase letters, digits, hyphens).\n\
                 Invalid char: '{c}'. Try kebab-case like 'cicd-stub-fix'."
            );
        }
    }
    // Reject empty segments: catches leading hyphen, trailing hyphen, and
    // consecutive hyphens in one rule.
    if topic.split('-').any(|s| s.is_empty()) {
        anyhow::bail!(
            "Topic must not have leading, trailing, or consecutive hyphens.\n\
             Got '{topic}'. Try kebab-case like 'cicd-stub-fix'."
        );
    }
    Ok(())
}

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

    // Seed the singleton switchboard session if missing. The switchboard
    // is one accumulating log per harness, never date- or topic-keyed.
    let session_path = sessions_dir.join(format!("{SWITCHBOARD_TOPIC}.md"));
    if !session_path.exists() {
        let content = format!(
            "---\ncampaign: {SWITCHBOARD}\nentrypoint: \"Switchboard ready. First-glance: run `synthesist status` and ls campaigns/ so you know what's live. Then wait for the human's intent.\"\n---\n\n\
             # Switchboard\n\n\
             ## Log\n\
             Switchboard scaffolded.\n"
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
pub fn scaffold_campaign_stub(harness_dir: &Path, campaign: &str, topic: &str) -> Result<PathBuf> {
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

    // Seed the topic's session file with a bootstrap entrypoint so Claude
    // knows to run the onboarding conversation on first response.
    let session_path = sessions_dir.join(format!("{topic}.md"));
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
             # {topic}\n\n\
             ## Log\n\
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

/// Find or scaffold a session file for the given campaign and topic.
/// If a file at `campaigns/<campaign>/sessions/<topic>.md` exists, returns
/// it. Otherwise scaffolds from `campaigns/TEMPLATE/sessions/TEMPLATE.md`
/// (or a built-in fallback) and returns the new path.
pub fn resolve_or_scaffold_session(
    harness_dir: &Path,
    campaign: &str,
    topic: &str,
) -> Result<PathBuf> {
    let campaign_dir = harness_dir.join("campaigns").join(campaign);
    let sessions_dir = campaign_dir.join("sessions");
    let path = sessions_dir.join(format!("{topic}.md"));
    if path.is_file() {
        return Ok(path);
    }

    fs::create_dir_all(&sessions_dir)?;
    let template_path = harness_dir
        .join("campaigns")
        .join("TEMPLATE")
        .join("sessions")
        .join("TEMPLATE.md");
    let content = if template_path.is_file() {
        let tpl = fs::read_to_string(&template_path)?;
        // `<date>[-<suffix>]` is the legacy placeholder kept here so existing
        // TEMPLATE.md files in user harnesses keep working without a rewrite.
        tpl.replace("<slug>", campaign)
            .replace("<topic>", topic)
            .replace("<date>[-<suffix>]", topic)
    } else {
        format!("---\ncampaign: {campaign}\nentrypoint: \"\"\n---\n\n# {topic}\n\n## Log\n\n")
    };
    fs::write(&path, content)?;
    Ok(path)
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
    fn resolve_or_scaffold_creates_file_at_topic() {
        let tmp = tempfile::tempdir().unwrap();
        let harness_dir = tmp.path();
        let campaign_dir = harness_dir.join("campaigns").join("gkg");
        fs::create_dir_all(&campaign_dir).unwrap();
        fs::write(
            campaign_dir.join("campaign.md"),
            "---\npaths: []\n---\n\n# gkg\n",
        )
        .unwrap();

        let path = resolve_or_scaffold_session(harness_dir, "gkg", "topic-flag").unwrap();
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "topic-flag.md");
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("campaign: gkg"));
        assert!(contents.contains("# topic-flag"));
    }

    #[test]
    fn resolve_or_scaffold_attaches_to_existing_topic() {
        let tmp = tempfile::tempdir().unwrap();
        let harness_dir = tmp.path();
        let sessions_dir = harness_dir.join("campaigns").join("gkg").join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        let existing = sessions_dir.join("retrieval.md");
        fs::write(
            &existing,
            "---\ncampaign: gkg\nentrypoint: x\n---\n\n# retrieval\n",
        )
        .unwrap();

        let path = resolve_or_scaffold_session(harness_dir, "gkg", "retrieval").unwrap();
        assert_eq!(path, existing);
    }

    #[test]
    fn resolve_or_scaffold_does_not_match_legacy_dated_files() {
        // Legacy `2026-04-24-cicd.md` files do not satisfy a `2026-04-24`
        // topic lookup. The new world is exact-match only.
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
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "2026-04-24.md");
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
        assert!(err.contains("Topic required"));
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
    fn scaffold_switchboard_creates_singleton() {
        let tmp = tempfile::tempdir().unwrap();
        let harness_dir = tmp.path();
        scaffold_switchboard(harness_dir).unwrap();

        let session_path = harness_dir
            .join("campaigns")
            .join(SWITCHBOARD)
            .join("sessions")
            .join(format!("{SWITCHBOARD_TOPIC}.md"));
        assert!(session_path.is_file(), "switchboard.md should exist");

        let contents = fs::read_to_string(&session_path).unwrap();
        assert!(contents.contains(&format!("campaign: {SWITCHBOARD}")));
        assert!(contents.contains("# Switchboard"));
    }

    #[test]
    fn scaffold_campaign_stub_writes_topic_keyed_session() {
        let tmp = tempfile::tempdir().unwrap();
        let harness_dir = tmp.path();
        scaffold_campaign_stub(harness_dir, "gkg", "retrieval-precision").unwrap();

        let campaign_md = harness_dir
            .join("campaigns")
            .join("gkg")
            .join("campaign.md");
        assert!(campaign_md.is_file());

        let session_path = harness_dir
            .join("campaigns")
            .join("gkg")
            .join("sessions")
            .join("retrieval-precision.md");
        assert!(session_path.is_file(), "topic-keyed session should exist");
        let body = fs::read_to_string(&session_path).unwrap();
        assert!(body.contains("campaign: gkg"));
        assert!(body.contains("# retrieval-precision"));
        assert!(body.contains("Bootstrap campaign 'gkg'"));
    }

    #[test]
    fn scaffold_switchboard_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let harness_dir = tmp.path();
        scaffold_switchboard(harness_dir).unwrap();
        let session_path = harness_dir
            .join("campaigns")
            .join(SWITCHBOARD)
            .join("sessions")
            .join(format!("{SWITCHBOARD_TOPIC}.md"));
        fs::write(&session_path, "custom content").unwrap();

        scaffold_switchboard(harness_dir).unwrap();
        assert_eq!(fs::read_to_string(&session_path).unwrap(), "custom content");
    }
}
