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

use crate::config::Layout;

/// Campaign frontmatter (`campaigns/<campaign>/campaign.md`).
#[derive(Debug, Default, Deserialize)]
pub struct Campaign {
    /// Classification slug (was the middle segment in the old 3-level name).
    /// Surfaced by the chooser (W5) and inherited by `shard` (W9).
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub synthesist_trees: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    /// Lineage: the parent campaign this one was sharded out of, if any.
    /// Consumed by the chooser grouping (W5) and `shard` (W9).
    #[serde(default)]
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

impl crate::config::Layout {
    /// `<repo-dir>/<campaigns_dir>/<campaign>/`.
    pub fn campaign_dir(&self, repo_dir: &Path, campaign: &str) -> PathBuf {
        repo_dir.join(&self.campaigns_dir).join(campaign)
    }
    /// `<repo-dir>/<campaigns_dir>/<campaign>/<campaign_file>`.
    pub fn campaign_md_path(&self, repo_dir: &Path, campaign: &str) -> PathBuf {
        self.campaign_dir(repo_dir, campaign).join(&self.campaign_file)
    }
    /// `<repo-dir>/<campaigns_dir>/<campaign>/<log_file>`.
    pub fn log_md_path(&self, repo_dir: &Path, campaign: &str) -> PathBuf {
        self.campaign_dir(repo_dir, campaign).join(&self.log_file)
    }
}


/// The SOLE raw file-read primitive. Every production file read routes through
/// here (directly, or via `load_optional` / `load_campaign` / `load_log`), so
/// `fs::read_to_string` appears nowhere else in the crate outside tests -- the
/// dev lint `scripts/lint-reads.sh` enforces it. Centralizing the read is what
/// lets each call site state its fail-loud-vs-degrade intent EXPLICITLY (`?` to
/// fail loud, a deliberate `.ok()` for a best-effort probe, `load_optional` to
/// split absent from present-but-broken) instead of the ad-hoc
/// `read_to_string(..).ok()` sites that silently swallowed real errors (#10/#11).
pub fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

pub fn load_campaign(path: &Path) -> Result<(Campaign, String)> {
    let content = read_text(path)?;
    let (fm, body) = split_frontmatter(&content)
        .with_context(|| format!("No YAML frontmatter in {}", path.display()))?;
    let campaign: Campaign = serde_yaml_ng::from_str(fm)
        .with_context(|| format!("Failed to parse campaign frontmatter: {}", path.display()))?;
    Ok((campaign, body.to_string()))
}

pub fn load_log(path: &Path) -> Result<(Log, String)> {
    let content = read_text(path)?;
    let (fm, body) = split_frontmatter(&content)
        .with_context(|| format!("No YAML frontmatter in {}", path.display()))?;
    let log: Log = serde_yaml_ng::from_str(fm)
        .with_context(|| format!("Failed to parse log frontmatter: {}", path.display()))?;
    Ok((log, body.to_string()))
}

/// Load an OPTIONAL campaign/log file, distinguishing ABSENT from
/// PRESENT-BUT-UNPARSEABLE.
///
/// - Absent file -> `Ok(None)`: a legitimate degrade (e.g. an
///   archived-but-still-running session whose log body is gone).
/// - Present + parses -> `Ok(Some(..))`.
/// - Present but unreadable / unparseable -> `Err`: a one-character
///   frontmatter typo must FAIL LOUD rather than silently strip a live
///   session's composed prompt and `--add-dir` paths on the next
///   recycle/upgrade (issue #11), mirroring the resolver extension's
///   fail-closed contract. A `stat` that cannot even determine existence
///   also fails loud rather than being treated as absent.
pub fn load_optional<T>(
    path: &Path,
    load: impl FnOnce(&Path) -> Result<T>,
) -> Result<Option<T>> {
    match path.try_exists() {
        Ok(false) => Ok(None),
        Ok(true) => load(path).map(Some),
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("cannot stat {}", path.display())))
        }
    }
}

/// Resolve `<repo-dir>/campaigns/<campaign>/campaign.md`, erroring if
/// the campaign does not exist.
pub fn campaign_file(layout: &Layout, repo_dir: &Path, campaign: &str) -> Result<PathBuf> {
    let path = layout.campaign_md_path(repo_dir, campaign);
    if !path.is_file() {
        anyhow::bail!("Campaign '{campaign}' not found at {}.", path.display());
    }
    Ok(path)
}

/// Lightweight metadata for one campaign on disk, as surfaced by the chooser
/// (W5), the migration tool (W6), and `shard` (W9). Cheap to build: only the
/// frontmatter fields the caller groups/sorts on, not the bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignInfo {
    /// The campaign slug (its directory name under `campaigns/`).
    pub name: String,
    /// Classification slug from frontmatter (`category:`), empty if unset.
    pub category: String,
    /// Lineage: the hub campaign this was sharded out of, if any.
    pub sharded_from: Option<String>,
}

/// Discover every campaign in a repo by scanning `campaigns/*/campaign.md`.
///
/// Returns one `CampaignInfo` per campaign directory that has a readable
/// `campaign.md`, sorted by name. A directory without a parseable
/// `campaign.md` is skipped (it isn't an onboarded campaign yet). Missing
/// `campaigns/` is not an error -- a repo with no campaigns yields an empty
/// list.
pub fn list_campaigns(layout: &Layout, repo_dir: &Path) -> Result<Vec<CampaignInfo>> {
    let campaigns_dir = repo_dir.join(&layout.campaigns_dir);
    if !campaigns_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in fs::read_dir(&campaigns_dir)
        .with_context(|| format!("Failed to read {}", campaigns_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue, // non-UTF8 dir name -- not a valid slug
        };
        if name == layout.archive_dir {
            continue; // archived campaigns are hidden from the launcher
        }
        let md = layout.campaign_md_path(repo_dir, &name);
        // Split absent from present-but-broken (load_optional's contract): a dir
        // with NO campaign.md isn't an onboarded campaign -> skip silently. A
        // campaign.md that EXISTS but won't parse is a broken onboarded campaign
        // -> WARN loudly so it can't silently VANISH from the chooser, but don't
        // abort the whole scan for one typo (the chooser stays usable).
        match load_optional(&md, load_campaign) {
            Ok(Some((campaign, _body))) => out.push(CampaignInfo {
                name,
                category: campaign.category,
                sharded_from: campaign.sharded_from,
            }),
            Ok(None) => {} // no campaign.md here -- not an onboarded campaign
            Err(e) => crate::ui::warn(&format!("skipping campaign '{name}': {e:#}")),
        }
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}


/// Archive a campaign: move `campaigns/<campaign>/` to
/// `campaigns/archive/<campaign>/`. Reversible (it's a move), and the chooser
/// stops listing it. Errors if the campaign is missing or already archived.
pub fn archive_campaign(layout: &Layout, repo_dir: &Path, campaign: &str) -> Result<PathBuf> {
    let src = layout.campaign_dir(repo_dir, campaign);
    if !src.is_dir() {
        anyhow::bail!("Campaign '{campaign}' not found at {}.", src.display());
    }
    let archive_root = repo_dir.join(&layout.campaigns_dir).join(&layout.archive_dir);
    fs::create_dir_all(&archive_root)?;
    let dest = archive_root.join(campaign);
    if dest.exists() {
        anyhow::bail!("'{campaign}' is already archived at {}.", dest.display());
    }
    fs::rename(&src, &dest)
        .with_context(|| format!("Failed to archive {} -> {}", src.display(), dest.display()))?;
    Ok(dest)
}

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
pub fn scaffold_switchboard(layout: &Layout, repo_dir: &Path) -> Result<PathBuf> {
    let dir = layout.campaign_dir(repo_dir, &layout.switchboard_slug);
    fs::create_dir_all(&dir)?;

    let campaign_md = layout.campaign_md_path(repo_dir, &layout.switchboard_slug);
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
    let log_md = layout.log_md_path(repo_dir, &layout.switchboard_slug);
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
pub fn scaffold_campaign_stub(layout: &Layout, repo_dir: &Path, campaign: &str) -> Result<PathBuf> {
    let dir = layout.campaign_dir(repo_dir, campaign);
    fs::create_dir_all(&dir)?;

    let campaign_content = format!(
        "---\ncategory: \"\"\nsynthesist_trees: []\npaths: []\n---\n\n\
         # {campaign}\n\n\
         ## What this is\n\
         (pending -- the tool will prompt the human on first launch)\n\n\
         ## How to behave\n\
         (pending)\n"
    );
    let campaign_md = layout.campaign_md_path(repo_dir, campaign);
    fs::write(&campaign_md, campaign_content)?;

    // Seed the log with a bootstrap entrypoint so the tool knows to run the
    // onboarding conversation on first response.
    let log_md = layout.log_md_path(repo_dir, campaign);
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

/// Shard a hub campaign into a new sibling campaign.
///
/// Creates `campaigns/<new>/` carrying the hub's `category:` (so siblings
/// classify together) and `sharded_from: <hub>` lineage, and seeds `log.md`
/// with a pointer back to the hub. This is the primitive behind "use the ncbi
/// session to scope a problem, then shard it out as its own campaign": depth
/// that would have been a third name segment becomes a lineage *link* between
/// two-level siblings.
///
/// Errors if the hub has no `campaign.md` (can't shard a non-campaign) or the
/// new slug already exists (never clobber). The new slug must already be
/// validated by the caller.
pub fn scaffold_shard(layout: &Layout, repo_dir: &Path, hub: &str, new: &str) -> Result<PathBuf> {
    let hub_md = layout.campaign_md_path(repo_dir, hub);
    let (hub_campaign, _body) = load_campaign(&hub_md)
        .with_context(|| format!("Cannot shard: hub campaign '{hub}' not found or unreadable"))?;

    let new_md = layout.campaign_md_path(repo_dir, new);
    if new_md.exists() {
        anyhow::bail!(
            "Campaign '{new}' already exists at {}. Pick a different shard slug.",
            new_md.display()
        );
    }

    let dir = layout.campaign_dir(repo_dir, new);
    fs::create_dir_all(&dir)?;

    let category = if hub_campaign.category.is_empty() {
        String::new()
    } else {
        hub_campaign.category.clone()
    };
    let campaign_content = format!(
        "---\ncategory: \"{category}\"\nsharded_from: {hub}\nsynthesist_trees: []\npaths: []\n---\n\n\
         # {new}\n\n\
         ## What this is\n\
         Sharded out of the `{hub}` campaign to focus on this specific topic. \
         Lineage is recorded in `sharded_from` so the chooser groups this \
         under its hub. (Fill in the specifics on first launch.)\n\n\
         ## How to behave\n\
         (pending -- inherited from the {hub} hub; refine for this topic)\n"
    );
    fs::write(&new_md, campaign_content)?;

    let log_md = layout.log_md_path(repo_dir, new);
    let entrypoint = format!(
        "Sharded from the '{hub}' hub campaign. This session focuses one topic \
         that crystallized inside {hub}. First action: confirm the scope with \
         the human in one exchange, write specifics into campaign.md via Edit \
         (paths, trees), then proceed. The hub remains the place for \
         cross-cutting {hub} work."
    );
    let log_content = format!(
        "---\nentrypoint: \"{entrypoint}\"\n---\n\n\
         # {new}\n\n\
         ## Log\n\
         Sharded from `{hub}`.\n"
    );
    fs::write(&log_md, log_content)?;

    Ok(new_md)
}

/// Find or scaffold the log file for the given campaign.
/// If `campaigns/<campaign>/log.md` exists, returns it. Otherwise scaffolds
/// the campaign dir + a minimal log.md and returns the new path.
pub fn resolve_or_scaffold_session(
    layout: &Layout,
    repo_dir: &Path,
    campaign: &str,
) -> Result<PathBuf> {
    let log_md = layout.log_md_path(repo_dir, campaign);
    if log_md.is_file() {
        return Ok(log_md);
    }

    let dir = layout.campaign_dir(repo_dir, campaign);
    fs::create_dir_all(&dir)?;
    let content = format!("---\nentrypoint: \"\"\n---\n\n# {campaign}\n\n## Log\n\n");
    fs::write(&log_md, content)?;
    Ok(log_md)
}

/// Compose the system prompt addition: the campaign's what/how plus a stable
/// *pointer* to the durable on-disk state.
///
/// Deliberately does NOT inline the (growing) log body. The log is what bloats
/// the prompt -- and a fat prompt is resent every turn, burning the context
/// window that forces compaction, while also going stale the moment `log.md`
/// is updated. Instead the prompt carries the one-line `entrypoint` (the
/// movable "where we are" pointer) and the absolute paths to re-read. Because
/// the system prompt survives `/compact`, the re-read instruction persists
/// even as the conversation summary decays, so the agent can re-orient from
/// the current files in seconds rather than rediscovering from a lossy summary.
pub fn compose_prompt(
    campaign: &str,
    campaign_body: &str,
    entrypoint: &str,
    campaign_md_path: &Path,
    log_md_path: &Path,
) -> String {
    let ep = entrypoint.trim();
    let ep_line = if ep.is_empty() {
        "(none yet -- read log.md)"
    } else {
        ep
    };
    format!(
        "# Campaign: {campaign}\n\n{}\n\n---\n\n## Current pointer\n\n\
         entrypoint: {ep_line}\n\n\
         This system prompt is a STABLE POINTER, not a snapshot. Your durable, \
         current state lives on disk at the paths below. Re-read them before \
         acting when you are unsure, and ALWAYS immediately after a /compact -- \
         the conversation summary is lossy, so these files are the source of \
         truth for where the work stands:\n\
         - {}\n\
         - {}\n",
        campaign_body.trim(),
        campaign_md_path.display(),
        log_md_path.display(),
    )
}

/// Expand `~` in a path string to the user's home directory.
pub fn expand_home(path: &str) -> String {
    shellexpand::tilde(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test shims: the layout-dependent ops take a `Layout` in the real API.
    // Tests exercise the built-in DEFAULT layout, so adapt here rather than
    // threading `Layout::default()` through every test body.
    const SWITCHBOARD: &str = "switchboard";
    fn campaign_dir(r: &Path, c: &str) -> PathBuf {
        Layout::default().campaign_dir(r, c)
    }
    fn campaign_md_path(r: &Path, c: &str) -> PathBuf {
        Layout::default().campaign_md_path(r, c)
    }
    fn log_md_path(r: &Path, c: &str) -> PathBuf {
        Layout::default().log_md_path(r, c)
    }
    fn scaffold_switchboard(r: &Path) -> Result<PathBuf> {
        super::scaffold_switchboard(&Layout::default(), r)
    }
    fn scaffold_campaign_stub(r: &Path, c: &str) -> Result<PathBuf> {
        super::scaffold_campaign_stub(&Layout::default(), r, c)
    }
    fn scaffold_shard(r: &Path, h: &str, n: &str) -> Result<PathBuf> {
        super::scaffold_shard(&Layout::default(), r, h, n)
    }
    fn resolve_or_scaffold_session(r: &Path, c: &str) -> Result<PathBuf> {
        super::resolve_or_scaffold_session(&Layout::default(), r, c)
    }
    fn archive_campaign(r: &Path, c: &str) -> Result<PathBuf> {
        super::archive_campaign(&Layout::default(), r, c)
    }
    fn list_campaigns(r: &Path) -> Result<Vec<CampaignInfo>> {
        super::list_campaigns(&Layout::default(), r)
    }

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
    fn compose_prompt_is_a_pointer_not_a_log_snapshot() {
        let cmd = Path::new("/repo/campaigns/gkg/campaign.md");
        let log = Path::new("/repo/campaigns/gkg/log.md");
        let out = compose_prompt("gkg", "## What\ngkg stuff", "ship the thing", cmd, log);
        assert!(out.contains("Campaign: gkg"));
        assert!(out.contains("gkg stuff"));
        // The entrypoint pointer is inline...
        assert!(out.contains("entrypoint: ship the thing"));
        // ...the re-read directive and absolute paths are present...
        assert!(out.contains("after a /compact"));
        assert!(out.contains("/repo/campaigns/gkg/campaign.md"));
        assert!(out.contains("/repo/campaigns/gkg/log.md"));
        // ...and the (growing) log body is NOT snapshotted in.
        assert!(!out.contains("# Log"));
    }

    #[test]
    fn compose_prompt_handles_empty_entrypoint() {
        let cmd = Path::new("/r/campaign.md");
        let log = Path::new("/r/log.md");
        let out = compose_prompt("x", "body", "", cmd, log);
        assert!(out.contains("(none yet -- read log.md)"));
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
    fn scaffold_shard_inherits_category_and_sets_lineage() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        // Hub campaign with a category.
        scaffold_campaign_stub(repo_dir, "ncbi").unwrap();
        fs::write(
            campaign_md_path(repo_dir, "ncbi"),
            "---\ncategory: account\npaths: []\n---\n\n# ncbi\n",
        )
        .unwrap();

        scaffold_shard(repo_dir, "ncbi", "ncbi-retrieval").unwrap();

        let (shard, _) = load_campaign(&campaign_md_path(repo_dir, "ncbi-retrieval")).unwrap();
        assert_eq!(shard.category, "account");
        assert_eq!(shard.sharded_from.as_deref(), Some("ncbi"));

        let log = fs::read_to_string(log_md_path(repo_dir, "ncbi-retrieval")).unwrap();
        assert!(log.contains("Sharded from `ncbi`"));
    }

    #[test]
    fn scaffold_shard_rejects_missing_hub() {
        let tmp = tempfile::tempdir().unwrap();
        let err = scaffold_shard(tmp.path(), "ghost", "child")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not found"));
    }

    #[test]
    fn scaffold_shard_rejects_existing_target() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        scaffold_campaign_stub(repo_dir, "hub").unwrap();
        scaffold_campaign_stub(repo_dir, "taken").unwrap();
        let err = scaffold_shard(repo_dir, "hub", "taken")
            .unwrap_err()
            .to_string();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn list_campaigns_empty_when_no_campaigns_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(list_campaigns(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn list_campaigns_reads_metadata_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        scaffold_campaign_stub(repo_dir, "zeta").unwrap();
        scaffold_campaign_stub(repo_dir, "alpha").unwrap();
        // Give alpha a category and a shard lineage.
        fs::write(
            campaign_md_path(repo_dir, "alpha"),
            "---\ncategory: research\nsharded_from: zeta\npaths: []\n---\n\n# alpha\n",
        )
        .unwrap();

        let campaigns = list_campaigns(repo_dir).unwrap();
        assert_eq!(campaigns.len(), 2);
        // Sorted by name: alpha before zeta.
        assert_eq!(campaigns[0].name, "alpha");
        assert_eq!(campaigns[0].category, "research");
        assert_eq!(campaigns[0].sharded_from.as_deref(), Some("zeta"));
        assert_eq!(campaigns[1].name, "zeta");
        assert!(campaigns[1].sharded_from.is_none());
    }

    #[test]
    fn archive_campaign_moves_and_hides_from_listing() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        scaffold_campaign_stub(repo_dir, "stale-one").unwrap();
        scaffold_campaign_stub(repo_dir, "keep-me").unwrap();
        assert_eq!(list_campaigns(repo_dir).unwrap().len(), 2);

        let dest = archive_campaign(repo_dir, "stale-one").unwrap();
        assert!(dest.ends_with("campaigns/archive/stale-one"));
        assert!(dest.join("campaign.md").is_file(), "content moved, not lost");
        // No longer in the listing; the archive/ dir itself is skipped too.
        let names: Vec<String> = list_campaigns(repo_dir).unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["keep-me"]);
    }

    #[test]
    fn archive_campaign_rejects_missing_and_double_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        assert!(archive_campaign(repo_dir, "ghost").unwrap_err().to_string().contains("not found"));
        scaffold_campaign_stub(repo_dir, "x").unwrap();
        archive_campaign(repo_dir, "x").unwrap();
        // re-creating x then archiving again collides with the archived copy.
        scaffold_campaign_stub(repo_dir, "x").unwrap();
        assert!(archive_campaign(repo_dir, "x").unwrap_err().to_string().contains("already archived"));
    }

    #[test]
    fn list_campaigns_skips_dirs_without_campaign_md() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        scaffold_campaign_stub(repo_dir, "real").unwrap();
        // A bare dir with no campaign.md is not a campaign.
        fs::create_dir_all(campaign_dir(repo_dir, "not-a-campaign")).unwrap();

        let campaigns = list_campaigns(repo_dir).unwrap();
        assert_eq!(campaigns.len(), 1);
        assert_eq!(campaigns[0].name, "real");
    }

    #[test]
    fn list_campaigns_skips_unparseable_campaign_md_without_aborting() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path();
        scaffold_campaign_stub(repo_dir, "good").unwrap();
        // A PRESENT campaign.md with no frontmatter is unparseable. It must not
        // abort the whole scan (one typo can't break the chooser) and must not
        // silently masquerade as a launchable campaign -- it is skipped (and
        // warned about via ui::warn, to stderr, so it does not silently vanish).
        fs::create_dir_all(campaign_dir(repo_dir, "broken")).unwrap();
        fs::write(campaign_md_path(repo_dir, "broken"), "no frontmatter at all\n").unwrap();

        let campaigns =
            list_campaigns(repo_dir).expect("scan does not abort on one broken campaign");
        let names: Vec<&str> = campaigns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["good"], "broken campaign skipped, good one still listed");
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

    #[test]
    fn custom_layout_overrides_paths() {
        let layout = Layout {
            campaigns_dir: "initiatives".into(),
            campaign_file: "spec.md".into(),
            log_file: "journal.md".into(),
            archive_dir: "attic".into(),
            switchboard_slug: "hub".into(),
        };
        let r = Path::new("/repo");
        assert_eq!(
            layout.campaign_dir(r, "x"),
            Path::new("/repo/initiatives/x")
        );
        assert_eq!(
            layout.campaign_md_path(r, "x"),
            Path::new("/repo/initiatives/x/spec.md")
        );
        assert_eq!(
            layout.log_md_path(r, "x"),
            Path::new("/repo/initiatives/x/journal.md")
        );
        // and the default reproduces the built-in 2-level model
        let d = Layout::default();
        assert_eq!(d.campaign_md_path(r, "x"), Path::new("/repo/campaigns/x/campaign.md"));
    }
}
