//! W6: migrate a repo's on-disk `campaigns/` tree from the old 3-level layout
//! to the 2-level repo/campaign model.
//!
//! Old layout (one dir per *category*, each holding many session topics):
//! ```text
//! campaigns/<category>/campaign.md          # category metadata (trees, paths)
//! campaigns/<category>/sessions/<topic>.md  # one per initiative (frontmatter + log)
//! campaigns/<category>/sessions/archive/    # stale, pruned on migrate
//! ```
//! New layout (one dir per *campaign* == initiative):
//! ```text
//! campaigns/<campaign>/campaign.md          # category: <category> in frontmatter
//! campaigns/<campaign>/log.md               # entrypoint + log body
//! ```
//!
//! The migration is filesystem-only and reversible via git: it reads each old
//! session topic, writes a new `campaigns/<topic>/{campaign.md,log.md}` (the
//! topic becomes the campaign slug; the old category becomes frontmatter,
//! inheriting the category's `synthesist_trees`/`paths`), then removes the old
//! category dir. It does NOT touch `state.json` or live tmux sessions -- that
//! cutover stays a human-gated step (`muxr save` -> migrate -> edit config
//! `[harnesses]`->`[repos]` -> `muxr restore`), which is why a real run prints
//! the session-name rewrites the human applies.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::primitives;

/// One old session topic to be migrated into its own campaign.
#[derive(Debug, PartialEq, Eq)]
pub struct PlannedMove {
    /// Old category (the middle segment; leading underscore stripped).
    pub category: String,
    /// New campaign slug. Usually the session-file stem; a collision or an
    /// invalid raw stem may make it `<category>-<stem>` or a sanitized form.
    pub topic: String,
    /// The original session-file stem, for the old->new rename display.
    pub orig_topic: String,
    /// Old `sessions/<topic>.md`.
    pub source: PathBuf,
    /// Old `campaigns/<category>/campaign.md`, for inheriting trees/paths.
    pub category_md: Option<PathBuf>,
}

/// The full set of changes a migration would make to one repo.
#[derive(Debug, Default)]
pub struct Plan {
    pub moves: Vec<PlannedMove>,
    /// Old category dirs to remove once their topics are migrated.
    pub old_dirs: Vec<PathBuf>,
    /// Archived session files found (pruned unless `keep_archives`).
    pub archives: Vec<PathBuf>,
    /// Human-readable reasons we skipped a dir/topic.
    pub skips: Vec<String>,
}

/// Options for executing a plan.
pub struct Opts {
    pub dry_run: bool,
    /// Move `sessions/archive/*` into a top-level `archive/` instead of
    /// dropping them.
    pub keep_archives: bool,
}

/// True if `dir` is an old-layout category: it has a `sessions/` subdir and is
/// not already a new-layout campaign (which has `log.md`).
fn is_old_category(dir: &Path) -> bool {
    dir.join("sessions").is_dir() && !dir.join("log.md").is_file()
}

/// Strip a leading underscore from a category name (`_switchboard` ->
/// `switchboard`) so the frontmatter value is a clean slug.
fn normalize_category(name: &str) -> String {
    name.strip_prefix('_').unwrap_or(name).to_string()
}

/// Coerce a raw session stem into a valid campaign slug, or `None` if it
/// can't be salvaged. Already-valid stems pass through unchanged; otherwise
/// non-`[a-z0-9-]` runs collapse to a single hyphen (so `v0.1-release` ->
/// `v0-1-release`) and the result is re-validated.
fn normalize_slug(raw: &str) -> Option<String> {
    if primitives::validate_topic(raw).is_ok() {
        return Some(raw.to_string());
    }
    let mut s: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_string();
    primitives::validate_topic(&s).is_ok().then_some(s)
}

/// Resolve the target slug for a session, avoiding collisions. Tries the
/// (sanitized) stem first; on collision falls back to `<category>-<stem>`;
/// returns `None` if neither is usable. `taken` is the set of slugs already
/// claimed (planned or existing on disk).
fn resolve_slug(
    stem: &str,
    category: &str,
    taken: &std::collections::HashSet<String>,
    campaigns_dir: &Path,
) -> Option<String> {
    let exists = |s: &str| taken.contains(s) || campaigns_dir.join(s).is_dir();
    if let Some(primary) = normalize_slug(stem)
        && !exists(&primary)
    {
        return Some(primary);
    }
    if let Some(prefixed) = normalize_slug(&format!("{category}-{stem}"))
        && !exists(&prefixed)
    {
        return Some(prefixed);
    }
    None
}

/// Build the migration plan for a repo (read-only; makes no changes).
pub fn plan(repo_dir: &Path) -> Result<Plan> {
    let campaigns_dir = repo_dir.join("campaigns");
    let mut plan = Plan::default();
    if !campaigns_dir.is_dir() {
        return Ok(plan);
    }

    // Collect category dirs first so we can detect target collisions against
    // the existing tree deterministically.
    let mut cat_dirs: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&campaigns_dir)
        .with_context(|| format!("Failed to read {}", campaigns_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        cat_dirs.push((name, entry.path()));
    }
    cat_dirs.sort_by(|a, b| a.0.cmp(&b.0));

    // Names that will exist after migration (existing new campaigns + planned
    // topics), to catch collisions.
    let mut planned_targets: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (cat_name, cat_path) in &cat_dirs {
        if !is_old_category(cat_path) {
            if cat_path.join("log.md").is_file() {
                // Already a new-layout campaign.
                planned_targets.insert(cat_name.clone());
            } else {
                plan.skips
                    .push(format!("{cat_name}: no sessions/ and no log.md -- left as-is"));
            }
            continue;
        }

        let category = normalize_category(cat_name);
        let category_md = {
            let p = cat_path.join("campaign.md");
            p.is_file().then_some(p)
        };
        let sessions = cat_path.join("sessions");

        // A category dir is only safe to remove if EVERY file under it migrates
        // cleanly. Any skip, unexpected dir, or stray non-session file leaves
        // un-migrated content behind, so we keep the dir and never delete it.
        let mut cat_clean = true;

        // First: the category dir must contain ONLY `campaign.md` and
        // `sessions/`. Anything else (e.g. a sibling `notes/` dir, a stray
        // file) lives outside the sessions scan and would be destroyed when
        // the dir is removed -- so its presence forces us to keep the dir.
        for top in fs::read_dir(cat_path)
            .with_context(|| format!("Failed to read {}", cat_path.display()))?
        {
            let top = top?;
            let nm = top.file_name();
            if nm != "campaign.md" && nm != "sessions" {
                plan.skips.push(format!(
                    "{cat_name}/{:?}: category-level content outside sessions/ -- dir KEPT",
                    nm
                ));
                cat_clean = false;
            }
        }

        for sub in fs::read_dir(&sessions)
            .with_context(|| format!("Failed to read {}", sessions.display()))?
        {
            let sub = sub?;
            let path = sub.path();
            let ftype = sub.file_type()?;
            if ftype.is_dir() {
                if sub.file_name() == "archive" {
                    for a in fs::read_dir(&path)? {
                        let a = a?;
                        if a.path().extension().and_then(|e| e.to_str()) == Some("md") {
                            plan.archives.push(a.path());
                        }
                    }
                } else {
                    plan.skips.push(format!(
                        "{cat_name}/sessions/{:?}: unexpected dir -- dir KEPT",
                        sub.file_name()
                    ));
                    cat_clean = false;
                }
                continue;
            }
            // A `<topic>.md` session file. A stray non-.md file blocks removal.
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                plan.skips.push(format!(
                    "{cat_name}/sessions/{:?}: not a .md session -- dir KEPT",
                    sub.file_name()
                ));
                cat_clean = false;
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                cat_clean = false;
                continue;
            };
            let stem = stem.to_string();

            match resolve_slug(&stem, &category, &planned_targets, &campaigns_dir) {
                Some(slug) => {
                    planned_targets.insert(slug.clone());
                    plan.moves.push(PlannedMove {
                        category: category.clone(),
                        topic: slug,
                        orig_topic: stem,
                        source: path,
                        category_md: category_md.clone(),
                    });
                }
                None => {
                    plan.skips.push(format!(
                        "{cat_name}/sessions/{stem}.md: no free slug (stem and '{category}-{stem}' both taken/invalid) -- dir KEPT"
                    ));
                    cat_clean = false;
                }
            }
        }

        // Only schedule removal when nothing was left behind.
        if cat_clean {
            plan.old_dirs.push(cat_path.clone());
        }
    }

    Ok(plan)
}

/// Serialize a new-layout `campaign.md`, inheriting category metadata.
fn render_campaign_md(category: &str, trees: &[String], paths: &[String], body: &str) -> String {
    let trees_yaml = if trees.is_empty() {
        " []".to_string()
    } else {
        format!(
            "\n{}",
            trees
                .iter()
                .map(|t| format!("  - {t}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let paths_yaml = if paths.is_empty() {
        " []".to_string()
    } else {
        format!(
            "\n{}",
            paths
                .iter()
                .map(|p| format!("  - {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let body = body.trim();
    format!(
        "---\ncategory: \"{category}\"\nsynthesist_trees:{trees_yaml}\npaths:{paths_yaml}\n---\n\n{body}\n"
    )
}

/// Serialize a new-layout `log.md` from the old session entrypoint + body.
fn render_log_md(topic: &str, entrypoint: &str, body: &str) -> String {
    let body = body.trim();
    let body = if body.is_empty() {
        format!("# {topic}\n\n## Log\n")
    } else {
        body.to_string()
    };
    // Escape embedded double quotes in the entrypoint for the YAML scalar.
    let ep = entrypoint.replace('"', "\\\"");
    format!("---\nentrypoint: \"{ep}\"\n---\n\n{body}\n")
}

/// Execute a plan. With `dry_run`, makes no changes (caller prints the plan).
pub fn execute(repo_dir: &Path, plan: &Plan, opts: &Opts) -> Result<()> {
    if opts.dry_run {
        return Ok(());
    }
    let campaigns_dir = repo_dir.join("campaigns");

    for mv in &plan.moves {
        // Inherit category trees/paths/body, best-effort.
        let (trees, paths, cat_body) = match &mv.category_md {
            Some(p) => match primitives::load_campaign(p) {
                Ok((c, body)) => (c.synthesist_trees, c.paths, body),
                Err(_) => (Vec::new(), Vec::new(), String::new()),
            },
            None => (Vec::new(), Vec::new(), String::new()),
        };
        let (log, log_body) = primitives::load_log(&mv.source)
            .unwrap_or((primitives::Log::default(), String::new()));

        let dest = campaigns_dir.join(&mv.topic);
        fs::create_dir_all(&dest)
            .with_context(|| format!("Failed to create {}", dest.display()))?;

        let body = if cat_body.trim().is_empty() {
            format!("# {}\n\n## What this is\n(migrated from category '{}')\n", mv.topic, mv.category)
        } else {
            cat_body
        };
        fs::write(
            dest.join("campaign.md"),
            render_campaign_md(&mv.category, &trees, &paths, &body),
        )?;
        fs::write(
            dest.join("log.md"),
            render_log_md(&mv.topic, &log.entrypoint, &log_body),
        )?;
    }

    // Archives: keep (move to top-level archive/) or drop (removed with the
    // old category dir below).
    if opts.keep_archives && !plan.archives.is_empty() {
        let archive_dir = repo_dir.join("archive");
        fs::create_dir_all(&archive_dir)?;
        for a in &plan.archives {
            let stem = a
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("archived.md");
            fs::rename(a, archive_dir.join(stem)).ok();
        }
    }

    // Remove old category dirs (campaign.md + sessions/). git is the safety net.
    for dir in &plan.old_dirs {
        fs::remove_dir_all(dir)
            .with_context(|| format!("Failed to remove old category dir {}", dir.display()))?;
    }

    Ok(())
}

/// Print a plan to stderr for `--dry-run` / pre-run review.
pub fn print_plan(repo_dir: &Path, plan: &Plan, opts: &Opts) {
    let repo = repo_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    eprintln!("migrate-layout plan for {} ({})", repo, repo_dir.display());
    eprintln!();
    if plan.moves.is_empty() {
        eprintln!("  (no old-layout categories to migrate)");
    }
    for mv in &plan.moves {
        let renamed = if mv.topic != mv.orig_topic { "  (slug changed)" } else { "" };
        eprintln!(
            "  campaigns/{}/sessions/{}.md  ->  campaigns/{}/{{campaign.md,log.md}}  [category: {}]{renamed}",
            mv.category, mv.orig_topic, mv.topic, mv.category
        );
        eprintln!(
            "      session rename: <harness>/{}/{}  ->  {}/{}",
            mv.category, mv.orig_topic, repo, mv.topic
        );
    }
    if !plan.archives.is_empty() {
        let verb = if opts.keep_archives {
            "move to archive/"
        } else {
            "DROP"
        };
        eprintln!();
        eprintln!("  archives ({}): {} file(s)", verb, plan.archives.len());
    }
    if !plan.old_dirs.is_empty() {
        eprintln!();
        eprintln!("  remove {} old category dir(s) after migrating", plan.old_dirs.len());
    }
    for s in &plan.skips {
        eprintln!("  skip: {s}");
    }
    eprintln!();
    if opts.dry_run {
        eprintln!("(dry run -- no changes made. Re-run without --dry-run to apply.)");
    } else {
        eprintln!("Applied. Next: edit config [harnesses] -> [repos], rewrite state.json");
        eprintln!("session names per the renames above, then `muxr restore`.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an old-layout repo tree for tests.
    fn old_repo(tmp: &Path) {
        let c = tmp.join("campaigns");
        // factory category with one session.
        let factory = c.join("factory");
        fs::create_dir_all(factory.join("sessions")).unwrap();
        fs::write(
            factory.join("campaign.md"),
            "---\nsynthesist_trees: [mytree]\npaths:\n  - ~/projects/webapp\n---\n\n# factory\n\n## What this is\nBuilding things.\n",
        )
        .unwrap();
        fs::write(
            factory.join("sessions").join("in-place-updates.md"),
            "---\ncampaign: factory\nentrypoint: \"do the thing\"\n---\n\n# in-place-updates\n\n## Log\nfirst entry\n",
        )
        .unwrap();
        // harness category with a session + an archive.
        let harness = c.join("harness");
        fs::create_dir_all(harness.join("sessions").join("archive")).unwrap();
        fs::write(
            harness.join("campaign.md"),
            "---\nsynthesist_trees: []\npaths: []\n---\n\n# harness\n",
        )
        .unwrap();
        fs::write(
            harness.join("sessions").join("tools.md"),
            "---\nentrypoint: \"\"\n---\n\n# tools\n",
        )
        .unwrap();
        fs::write(
            harness.join("sessions").join("archive").join("stale.md"),
            "---\nentrypoint: x\n---\n\n# stale\n",
        )
        .unwrap();
        // _switchboard category.
        let sb = c.join("_switchboard");
        fs::create_dir_all(sb.join("sessions")).unwrap();
        fs::write(sb.join("campaign.md"), "---\npaths: []\n---\n\n# sb\n").unwrap();
        fs::write(
            sb.join("sessions").join("switchboard.md"),
            "---\nentrypoint: \"sb ready\"\n---\n\n# switchboard\n",
        )
        .unwrap();
    }

    #[test]
    fn plan_finds_all_topics_and_archives() {
        let tmp = tempfile::tempdir().unwrap();
        old_repo(tmp.path());
        let p = plan(tmp.path()).unwrap();

        let mut topics: Vec<&str> = p.moves.iter().map(|m| m.topic.as_str()).collect();
        topics.sort();
        assert_eq!(topics, vec!["in-place-updates", "switchboard", "tools"]);
        // _switchboard category normalized to switchboard in frontmatter.
        let sb = p.moves.iter().find(|m| m.topic == "switchboard").unwrap();
        assert_eq!(sb.category, "switchboard");
        // one archive found.
        assert_eq!(p.archives.len(), 1);
        // three old category dirs to remove.
        assert_eq!(p.old_dirs.len(), 3);
    }

    #[test]
    fn execute_writes_new_layout_and_removes_old() {
        let tmp = tempfile::tempdir().unwrap();
        old_repo(tmp.path());
        let p = plan(tmp.path()).unwrap();
        execute(
            tmp.path(),
            &p,
            &Opts {
                dry_run: false,
                keep_archives: false,
            },
        )
        .unwrap();

        let c = tmp.path().join("campaigns");
        // New campaign exists with inherited category + trees.
        let (campaign, body) =
            primitives::load_campaign(&c.join("in-place-updates").join("campaign.md")).unwrap();
        assert_eq!(campaign.category, "factory");
        assert_eq!(campaign.synthesist_trees, vec!["mytree"]);
        assert_eq!(campaign.paths, vec!["~/projects/webapp"]);
        assert!(body.contains("Building things"));

        // Log carries the entrypoint + body.
        let (log, log_body) =
            primitives::load_log(&c.join("in-place-updates").join("log.md")).unwrap();
        assert_eq!(log.entrypoint, "do the thing");
        assert!(log_body.contains("first entry"));

        // Old category dirs are gone; archive dropped.
        assert!(!c.join("factory").exists());
        assert!(!c.join("harness").exists());
        assert!(!c.join("_switchboard").exists());
    }

    #[test]
    fn execute_dry_run_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        old_repo(tmp.path());
        let p = plan(tmp.path()).unwrap();
        execute(
            tmp.path(),
            &p,
            &Opts {
                dry_run: true,
                keep_archives: false,
            },
        )
        .unwrap();
        // Old layout still present, no new dirs.
        assert!(tmp.path().join("campaigns").join("factory").exists());
        assert!(!tmp.path().join("campaigns").join("in-place-updates").exists());
    }

    #[test]
    fn keep_archives_moves_them_to_top_level() {
        let tmp = tempfile::tempdir().unwrap();
        old_repo(tmp.path());
        let p = plan(tmp.path()).unwrap();
        execute(
            tmp.path(),
            &p,
            &Opts {
                dry_run: false,
                keep_archives: true,
            },
        )
        .unwrap();
        assert!(tmp.path().join("archive").join("stale.md").exists());
    }

    #[test]
    fn plan_skips_already_migrated() {
        let tmp = tempfile::tempdir().unwrap();
        let c = tmp.path().join("campaigns").join("already");
        fs::create_dir_all(&c).unwrap();
        fs::write(c.join("campaign.md"), "---\ncategory: x\npaths: []\n---\n\n# already\n").unwrap();
        fs::write(c.join("log.md"), "---\nentrypoint: \"\"\n---\n\n# already\n").unwrap();

        let p = plan(tmp.path()).unwrap();
        assert!(p.moves.is_empty());
        assert!(p.old_dirs.is_empty());
    }

    #[test]
    fn colliding_stems_fall_back_to_category_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let c = tmp.path().join("campaigns");
        // Two categories each with a bootstrap.md -- the classic collision.
        for cat in ["auth", "blog"] {
            let s = c.join(cat).join("sessions");
            fs::create_dir_all(&s).unwrap();
            fs::write(c.join(cat).join("campaign.md"), "---\npaths: []\n---\n").unwrap();
            fs::write(s.join("bootstrap.md"), "---\nentrypoint: \"\"\n---\n\n# bootstrap\n").unwrap();
        }
        let p = plan(tmp.path()).unwrap();
        let mut slugs: Vec<&str> = p.moves.iter().map(|m| m.topic.as_str()).collect();
        slugs.sort();
        // auth (sorted first) keeps `bootstrap`; blog falls back to `blog-bootstrap`.
        assert_eq!(slugs, vec!["blog-bootstrap", "bootstrap"]);
        // Both migrate -- nothing skipped, both dirs removable.
        assert_eq!(p.moves.len(), 2);
        assert_eq!(p.old_dirs.len(), 2);
        assert!(p.skips.is_empty());
    }

    #[test]
    fn sanitizes_dotted_invalid_stems() {
        let tmp = tempfile::tempdir().unwrap();
        let s = tmp.path().join("campaigns").join("rel").join("sessions");
        fs::create_dir_all(&s).unwrap();
        fs::write(tmp.path().join("campaigns/rel/campaign.md"), "---\npaths: []\n---\n").unwrap();
        fs::write(s.join("v0.1-release.md"), "---\nentrypoint: \"\"\n---\n\n# r\n").unwrap();
        let p = plan(tmp.path()).unwrap();
        assert_eq!(p.moves.len(), 1);
        assert_eq!(p.moves[0].topic, "v0-1-release");
        assert_eq!(p.moves[0].orig_topic, "v0.1-release");
    }

    #[test]
    fn category_with_unmigrated_file_is_not_removed() {
        // SAFETY: a category that still holds an un-migrated file (stray
        // non-.md, unexpected dir, or unresolvable slug) must NOT be scheduled
        // for removal, or execute() would delete that content.
        let tmp = tempfile::tempdir().unwrap();
        let s = tmp.path().join("campaigns").join("mix").join("sessions");
        fs::create_dir_all(&s).unwrap();
        fs::write(tmp.path().join("campaigns/mix/campaign.md"), "---\npaths: []\n---\n").unwrap();
        fs::write(s.join("good.md"), "---\nentrypoint: \"\"\n---\n\n# good\n").unwrap();
        fs::write(s.join("notes.txt"), "stray").unwrap(); // not a session
        let p = plan(tmp.path()).unwrap();
        // good.md migrates...
        assert_eq!(p.moves.len(), 1);
        // ...but the dir is KEPT because notes.txt would otherwise be lost.
        assert!(p.old_dirs.is_empty(), "dir with a stray file must not be removed");
        assert!(p.skips.iter().any(|s| s.contains("notes.txt")));

        // And execute proves the stray file survives.
        execute(tmp.path(), &p, &Opts { dry_run: false, keep_archives: false }).unwrap();
        assert!(s.join("notes.txt").exists(), "stray file must survive migration");
        assert!(tmp.path().join("campaigns/good/log.md").is_file());
    }

    #[test]
    fn category_level_sibling_dir_is_preserved() {
        // SAFETY: content beside sessions/ (e.g. a notes/ dir) is outside the
        // sessions scan; removing the category dir would destroy it, so the
        // dir must be kept and the sibling content must survive.
        let tmp = tempfile::tempdir().unwrap();
        let cat = tmp.path().join("campaigns").join("immutable");
        fs::create_dir_all(cat.join("sessions")).unwrap();
        fs::create_dir_all(cat.join("notes")).unwrap();
        fs::write(cat.join("campaign.md"), "---\npaths: []\n---\n").unwrap();
        fs::write(cat.join("sessions/deploy.md"), "---\nentrypoint: \"\"\n---\n\n# d\n").unwrap();
        fs::write(cat.join("notes/v4.md"), "untracked notes").unwrap();

        let p = plan(tmp.path()).unwrap();
        assert_eq!(p.moves.len(), 1, "deploy.md still migrates");
        assert!(p.old_dirs.is_empty(), "category with a notes/ sibling must be kept");

        execute(tmp.path(), &p, &Opts { dry_run: false, keep_archives: false }).unwrap();
        assert!(cat.join("notes/v4.md").exists(), "sibling notes/ must survive");
        assert!(tmp.path().join("campaigns/deploy/log.md").is_file());
    }

    #[test]
    fn render_campaign_md_roundtrips() {
        let md = render_campaign_md(
            "factory",
            &["mytree".to_string()],
            &["~/x".to_string(), "~/y".to_string()],
            "# t\n\nbody",
        );
        let (c, body) = {
            // reuse the primitives parser via a temp file path-free parse
            let parsed: primitives::Campaign =
                serde_yaml_ng::from_str(md.split("---").nth(1).unwrap()).unwrap();
            (parsed, md)
        };
        assert_eq!(c.category, "factory");
        assert_eq!(c.synthesist_trees, vec!["mytree"]);
        assert_eq!(c.paths, vec!["~/x", "~/y"]);
        assert!(body.contains("body"));
    }
}
