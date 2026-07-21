//! Campaign-session lifecycle: opening, launching, sharding, reorienting,
//! recycling, archiving, migrating, and the launch composition core. This is
//! the bulk of the 2.0 repo/campaign surface, factored out of main.rs.

use anyhow::{Context, Result};

use crate::config::{self, Config};
use crate::tmux::Tmux;
use crate::{migrate, primitives, remote, state, tool, ui};

/// Route a bare `muxr <repo> [<campaign>]` invocation.
///
/// `muxr <repo>` opens the repo switchboard; `muxr <repo> <campaign>` opens
/// (or scaffolds) that campaign. Remotes are dispatched to their own handler.
pub(crate) fn cmd_open_dispatch(
    tmux: &Tmux,
    args: &[String],
    tool_override: Option<&str>,
    fresh: bool,
) -> Result<()> {
    let config = Config::load()?;
    let name = &args[0];

    // Route to remote handler if this is a remote.
    if config.is_remote(name) {
        return cmd_open_remote(tmux, &config, name, args);
    }

    if !config.repos.contains_key(name) {
        let names = config.all_names().join(", ");
        anyhow::bail!("Unknown repo or remote: {name}\nKnown: {names}");
    }

    let dir = config.resolve_dir(name)?;
    let _ = tool_override;

    // No campaign arg -> route to the per-repo switchboard.
    if args.get(1).is_none() {
        primitives::scaffold_switchboard(&config.layout, &dir)?;
        return cmd_open(tmux, &config, name, &config.layout.switchboard_slug, false);
    }

    let campaign = args[1].as_str();
    primitives::validate_topic(campaign)?;
    if args.len() > 2 {
        let extras = args[2..].join(" ");
        anyhow::bail!(
            "Unexpected extra args: {extras}\n\
             Launch shape is: muxr <repo> <campaign>."
        );
    }

    cmd_open(tmux, &config, name, campaign, fresh)
}

/// Open or attach to a campaign session: muxr <repo> <campaign>
///
/// Resolves `campaigns/<campaign>/{campaign.md,log.md}`, scaffolding a stub
/// if missing. Composes the system prompt from the repo HARNESS prompt +
/// campaign body + log body; passes each campaign `paths:` entry as
/// `--add-dir`. The session is named `<repo>/<campaign>`.
pub(crate) fn cmd_open(
    tmux: &Tmux,
    config: &Config,
    repo_name: &str,
    campaign: &str,
    fresh: bool,
) -> Result<()> {
    let repo_dir = config.resolve_dir(repo_name)?;
    // If the campaign doesn't exist yet, scaffold a stub so the human can
    // onboard it in-flow. Keeps the launch single-command from the control
    // plane.
    if !config
        .layout
        .campaign_md_path(&repo_dir, campaign)
        .is_file()
    {
        primitives::scaffold_campaign_stub(&config.layout, &repo_dir, campaign)?;
    }
    let campaign_md = primitives::campaign_file(&config.layout, &repo_dir, campaign)?;
    let log_path = primitives::resolve_or_scaffold_session(&config.layout, &repo_dir, campaign)?;

    let session_name = format!("{repo_name}/{campaign}");

    if tmux.session_exists(&session_name) {
        ui::action(&format!("attaching to {session_name}"));
        tmux.attach(&session_name)?;
        return Ok(());
    }

    // Only reached when CREATING (not attaching). A leftover recycle sentinel
    // means the previous recycle of this session was interrupted before it could
    // reopen (crash, sleep, a killed detached watcher). Log it once and clear it --
    // muxr never auto-resurrects from a sentinel (ADR 0008: an explicit recycle is
    // required; a stale flag is a breadcrumb, not a trigger). Clearing only on the
    // create path avoids a concurrent `muxr open` (attach) racing a live recycle's
    // wait and eating its just-written sentinel. Best-effort: a state-dir error
    // here must not block opening the session.
    if Config::clear_recycle_sentinel(&session_name).unwrap_or(false) {
        ui::note(&format!(
            "cleared a leftover recycle sentinel for {session_name} \
             (a previous recycle was interrupted before it reopened)"
        ));
    }

    let tool = config.resolve_tool(repo_name, None);

    let (log_data, _log_body) = primitives::load_log(&log_path)?;

    // A dormant campaign is resumable: if `muxr save` recorded a conversation
    // id for this session name, relaunch with --resume so opening it picks up
    // where it left off. --fresh forces a new conversation that rehydrates from
    // the pointer (campaign.md + log.md) -- the recycle model; the prior
    // conversation stays on disk, recoverable via --resume.
    let resume_id = if fresh {
        None
    } else {
        state::SavedState::session_id_for(&session_name)?
    };

    // Build the launch command through the single composer (the one place that
    // materialises harness settings + composed prompt + campaign --add-dirs).
    let (tool_cmd, session_dir) =
        compose_launch_command(config, &session_name, resume_id.as_deref(), None, false)?;
    let (campaign_data, _campaign_body) = primitives::load_campaign(&campaign_md)?;

    // A single coherent launch card instead of scattered lines.
    ui::band(&session_name, "", config.color_for(repo_name));
    ui::detail("dir", &ui::abbreviate_home(&session_dir.to_string_lossy()));
    let mode = if resume_id.is_some() {
        " · resuming"
    } else if fresh {
        " · fresh"
    } else {
        ""
    };
    ui::detail("tool", &format!("{tool}{mode}"));
    if !campaign_data.synthesist_trees.is_empty() {
        ui::detail("trees", &campaign_data.synthesist_trees.join(", "));
    }
    if !campaign_data.paths.is_empty() {
        ui::detail(
            "paths",
            &format!("+{} --add-dir", campaign_data.paths.len()),
        );
    }
    let ep = log_data.entrypoint.lines().next().unwrap_or("").trim();
    if !ep.is_empty() {
        let ep = if ep.chars().count() > 72 {
            format!("{}…", ep.chars().take(72).collect::<String>())
        } else {
            ep.to_string()
        };
        ui::detail("entry", &ep);
    }

    config.run_pre_create_hooks(&session_dir);

    // Name the tool and flush before the blocking create+attach so the last
    // thing on screen is a clear state line, not a silent pause before tmux
    // takes over.
    ui::action(&format!("launching {tool}…"));
    tmux.create_session(
        &session_name,
        &session_dir,
        &tool_cmd,
        &config.session_env_for(&session_name),
        config
            .viewer_for(&session_name, session_dir.to_str().unwrap_or(""))
            .as_ref(),
    )?;
    tmux.attach(&session_name)?;
    Ok(())
}

/// Resolve a session name from an explicit arg or the current tmux session.
fn session_or_current(tmux: &Tmux, name: Option<&str>) -> Result<String> {
    match name {
        Some(n) => Ok(n.to_string()),
        None => tmux.current_session().context(
            "Not inside a muxr session. Pass <repo>/<campaign> explicitly, \
             or run from within the session's pane.",
        ),
    }
}

/// Shard the current (or named) campaign into a new sibling campaign, then
/// open it: muxr shard <new> [--repo <repo>] [--from <hub>]
///
/// The hub repo/campaign default to the session this is run from (via tmux
/// `#{session_name}`); `--repo`/`--from` override for out-of-session use. The
/// new campaign inherits the hub's category and records `sharded_from`
/// lineage, then launches as `<repo>/<new>`.
pub(crate) fn cmd_shard(
    tmux: &Tmux,
    new: &str,
    repo: Option<&str>,
    from: Option<&str>,
) -> Result<()> {
    let config = Config::load()?;

    // Resolve hub repo + campaign. Flags win; otherwise infer from the
    // current tmux session. Running from the control plane (or any non-campaign
    // session) without flags can't infer a hub -- guide the human to pass them.
    let (repo_name, hub) = match (repo, from) {
        (Some(r), Some(f)) => (r.to_string(), f.to_string()),
        _ => {
            let current = tmux.display_message("#{session_name}").unwrap_or_default();
            let inferred = parse_session(&current);
            match inferred {
                Some((r, c)) => (
                    repo.map(str::to_string).unwrap_or(r),
                    from.map(str::to_string).unwrap_or(c),
                ),
                None => anyhow::bail!(
                    "Cannot infer the hub campaign from here (current session: '{current}').\n\
                     Run `muxr shard <new>` from inside a campaign session, or pass\n\
                     `muxr shard <new> --repo <repo> --from <hub>` explicitly."
                ),
            }
        }
    };

    if new == hub {
        anyhow::bail!("Shard slug '{new}' is the same as the hub. Pick a distinct slug.");
    }
    primitives::validate_topic(new)?;

    if !config.repos.contains_key(&repo_name) {
        let names = config.all_names().join(", ");
        anyhow::bail!("Unknown repo: {repo_name}\nKnown: {names}");
    }
    let repo_dir = config.resolve_dir(&repo_name)?;

    let new_md = primitives::scaffold_shard(&config.layout, &repo_dir, &hub, new)?;
    eprintln!();
    eprintln!("Sharded '{hub}' -> '{new}' ({})", new_md.display());
    eprintln!();

    cmd_open(tmux, &config, &repo_name, new, false)
}

/// Re-anchor a live session to its current on-disk campaign + log: muxr reorient [name]
///
/// Sends a single-line nudge into the pane telling the agent to re-read its
/// campaign.md + log.md now. The system prompt already carries a standing
/// re-read pointer; this is the explicit, on-demand trigger (e.g. right after
/// a `/compact`). It does not relaunch and does not touch the conversation.
pub(crate) fn cmd_reorient(tmux: &Tmux, name: Option<&str>) -> Result<()> {
    let config = Config::load()?;

    let session = session_or_current(tmux, name)?;

    let (repo_name, campaign) = parse_session(&session)
        .with_context(|| format!("'{session}' is not a <repo>/<campaign> session"))?;
    let repo_dir = config.resolve_dir(&repo_name)?;
    let campaign_md = config.layout.campaign_md_path(&repo_dir, &campaign);
    let log_md = config.layout.log_md_path(&repo_dir, &campaign);

    if !campaign_md.is_file() {
        anyhow::bail!(
            "No campaign on disk for {session} (expected {}).",
            campaign_md.display()
        );
    }

    // One line (send-keys can't carry newlines reliably). The agent pulls the
    // current full state itself by re-reading the files.
    let nudge = format!(
        "[muxr reorient] Re-read your durable state NOW before continuing -- \
         the conversation context may be stale or compacted. Read {} and {}, \
         then resume from the current entrypoint.",
        campaign_md.display(),
        log_md.display()
    );

    tmux.send_text(&session, &nudge)?;
    eprintln!("Reoriented {session} (nudged to re-read campaign.md + log.md).");
    Ok(())
}

/// Recycle a session: flush state to the pointer, then reopen FRESH.
///
/// The deliberate alternative to compact-looping, and self-contained in muxr (no
/// external skill). muxr sends a FLUSH PROMPT into the pane asking the agent to
/// flush its state into `log.md` (a tight `entrypoint:` + a dated entry) and,
/// when done, write a sentinel file; muxr then WAITS for that sentinel -- the
/// agent's positive "flush complete" signal, never an inferred idle (ADR
/// 0008/0010) -- then drives `/exit`, waits for the pane to return to its shell,
/// and reopens FRESH (no resume) so it rehydrates from the pointer. The prior
/// conversation persists on disk (recoverable via --resume), so recycling never
/// destroys context. The flush prompt is muxr-owned and generic;
/// `[recycle].flush_prompt` overrides it (e.g. to compose an estate `durable`),
/// and `[recycle].flush_timeout_secs` tunes the abort deadline.
///
/// How long recycle waits for the tool pane to return to its shell after `/exit`
/// before reopening anyway. Generous: the `/exit` sent right after the flush
/// queues until any in-flight turn ends, so the return-to-shell can lag.
const RECYCLE_EXIT_TIMEOUT_SECS: u64 = 600;

pub(crate) fn cmd_recycle(tmux: &Tmux, name: Option<&str>) -> Result<()> {
    let config = Config::load()?;

    let session = session_or_current(tmux, name)?;
    let (repo_name, campaign) = parse_session(&session)
        .with_context(|| format!("'{session}' is not a <repo>/<campaign> session"))?;
    if !tmux.session_exists(&session) {
        anyhow::bail!("No live session named {session}.");
    }
    // No live tool to flush? If the pane is at a shell the agent has exited/
    // crashed -- refuse rather than type the multi-line flush prompt into a shell
    // (it would run as commands, and a stray redirect could even forge the
    // sentinel, driving a kill+recreate of a session the operator thought intact).
    if tool::session_at_shell(tmux, &session) {
        anyhow::bail!(
            "refusing to recycle {session}: its pane is at a shell, not a running tool \
             (the agent may have exited) -- reopen with `muxr {repo_name} {campaign}` instead."
        );
    }
    // Self-recycle guard: running `muxr recycle` NON-detached inside the very
    // session it targets deadlocks (muxr blocks the pane while waiting for a flush
    // the now-blocked agent cannot produce). Trip narrowly -- same session AND muxr
    // is the pane's foreground process; a DETACHED self-recycle leaves the agent
    // foreground, so this does not fire on the normal `setsid muxr recycle` path.
    if tmux.current_session().as_deref() == Some(session.as_str())
        && tmux.pane_current_command(&session).as_deref() == Some("muxr")
    {
        anyhow::bail!(
            "refusing to recycle {session}: this is the session running `muxr recycle` \
             non-detached -- it would deadlock. Run it detached \
             (`setsid muxr recycle {session}`) or from another pane."
        );
    }
    let repo_dir = config.resolve_dir(&repo_name)?;
    let log_md = config.layout.log_md_path(&repo_dir, &campaign);
    let campaign_md = config.layout.campaign_md_path(&repo_dir, &campaign);

    // Fail loud BEFORE the destructive flush+exit+kill (issue #11): if the
    // campaign or log file EXISTS but its frontmatter is unparseable, refuse
    // the recycle rather than kill the live session and relaunch it stripped of
    // its composed prompt and --add-dir paths. An ABSENT file is fine (an
    // archived-but-running session), so this only trips on genuine corruption.
    // The value is discarded: muxr does not compose a flush from the campaign
    // paths (the agent flushes in-context from the flush prompt, ADR 0008/0010);
    // this load is kept purely for its fail-loud side effect.
    primitives::load_optional(&campaign_md, primitives::load_campaign).with_context(|| {
        format!(
            "refusing to recycle {session}: campaign file present but unparseable: {} \
             (fix the frontmatter or remove the file)",
            campaign_md.display()
        )
    })?;
    primitives::load_optional(&log_md, primitives::load_log).with_context(|| {
        format!(
            "refusing to recycle {session}: log file present but unparseable: {} \
             (fix the frontmatter or remove the file)",
            log_md.display()
        )
    })?;

    let tool = config.resolve_tool(&repo_name, None);
    let tool_def = config.tool_for(&tool);

    // Recycle is a positive-signal handshake, never idle inference (ADR
    // 0008/0010 -- inference is what stranded/killed sessions across the 3.6.x
    // saga). muxr sends a FLUSH PROMPT into the pane asking the agent to flush its
    // working state to the durable pointer and, when done, write the sentinel
    // file; muxr then WAITS for that file -- the agent's positive "flush complete"
    // signal -- before it exits + reopens. The flush prompt is muxr-owned (a
    // generic default; the estate overrides it via `[recycle].flush_prompt` to
    // compose its `durable` skill), so recycle is self-contained and
    // runtime-agnostic -- no external skill required.
    let sentinel = Config::recycle_sentinel_path(&session)?;
    // Clear any stale sentinel from a prior interrupted recycle so we wait for a
    // FRESH signal. If the clear FAILS and a leftover remains, abort: a stale
    // sentinel would make wait_for_sentinel return instantly and drive /exit
    // before any flush ran.
    if let Err(e) = Config::clear_recycle_sentinel(&session) {
        anyhow::bail!(
            "recycle {session}: could not clear a stale sentinel ({}): {e} -- aborting to \
             avoid a false flush-done signal",
            sentinel.display()
        );
    }
    Config::ensure_state_dir()?;
    let flush_prompt =
        config.recycle_flush_prompt(&session, &repo_name, &campaign, &log_md, &sentinel);
    ui::action(&format!(
        "recycle {session}: flushing (waiting for the agent's done-signal)"
    ));
    tmux.send_text(&session, &flush_prompt)?;

    // Wait for the agent's positive flush-done signal. Generous cap: a flush can
    // push several repos. If it never arrives, ABORT without touching the session
    // -- fail-safe: no signal, no exit, so a busy or unresponsive session is
    // preserved rather than killed.
    if !tool::wait_for_sentinel(&sentinel, config.recycle.flush_timeout_secs) {
        anyhow::bail!(
            "recycle {session}: the agent did not signal flush-complete within {}s \
             (no {} written) -- session left untouched. Re-run when the flush can \
             complete, or flush + `muxr <repo> <campaign> --fresh` manually.",
            config.recycle.flush_timeout_secs,
            sentinel.display()
        );
    }

    // Flush signalled complete. Drive the exit keystroke -- the CLI input loop
    // receives it, unlike asking the agent to self-`/exit` (which cannot work and
    // hung recycle, #8).
    let exit_cmd = tool_def
        .as_ref()
        .and_then(|t| t.exit_command.clone())
        .unwrap_or_else(|| "/exit".to_string());
    ui::action(&format!("recycle {session}: exiting"));
    tmux.send_text(&session, &exit_cmd)?;
    // Wait for the tool pane to return to its shell (the tool exited). This is
    // the pane-current-command signal, robust across platforms and the pi `nono`
    // wrapper -- not the fragile process-tree PID match of pre-4.0 (ADR 0008).
    // Generous cap: the `/exit` sent right after the flush queues until any
    // in-flight turn ends, so the return-to-shell can lag.
    if !tool::wait_for_return_to_shell(tmux, &session, RECYCLE_EXIT_TIMEOUT_SECS) {
        ui::note("recycle: tool did not return to a shell in time; reopening anyway");
    }

    // Compose the relaunch BEFORE the destructive kill (issue #11 class). The
    // harness prompt files are read inside compose_launch_command, and since
    // 3.7.0 a missing/unreadable one FAILS LOUD -- doing that AFTER kill_session
    // would destroy the session and then fail to relaunch it. The log is already
    // flushed above, so composing here uses the updated pointer; a compose
    // failure now leaves the exited-but-unkilled session recoverable, matching
    // `upgrade`'s compose-before-exit invariant.
    let (tool_cmd, session_dir) = compose_launch_command(&config, &session, None, None, false)?;

    // Clean slate, then recreate fresh from the (now-updated) pointer.
    if tmux.session_exists(&session) {
        tmux.kill_session(&session)?;
    }
    config.run_pre_create_hooks(&session_dir);
    tmux.create_session(
        &session,
        &session_dir,
        &tool_cmd,
        &config.session_env_for(&session),
        config
            .viewer_for(&session, session_dir.to_str().unwrap_or(""))
            .as_ref(),
    )?;

    // Reopen succeeded: clear the sentinel so a subsequent `muxr open` doesn't
    // mistake this completed recycle for an interrupted one.
    let _ = Config::clear_recycle_sentinel(&session);

    ui::ok(&format!(
        "recycled {session} -- fresh conversation, rehydrated from the pointer"
    ));
    ui::note("previous conversation remains on disk (recoverable via --resume)");
    Ok(())
}

/// Archive a campaign out of the chooser: muxr archive <campaign> [--repo <repo>]
///
/// Resolves the repo (flag or current session), refuses if the campaign has a
/// live tmux session (don't archive running work), then moves it under
/// campaigns/archive/. Reversible.
pub(crate) fn cmd_archive(tmux: &Tmux, campaign: &str, repo: Option<&str>) -> Result<()> {
    let config = Config::load()?;

    let repo_name = match repo {
        Some(r) => r.to_string(),
        None => {
            let current = tmux.display_message("#{session_name}").unwrap_or_default();
            match parse_session(&current) {
                Some((r, _)) => r,
                None => anyhow::bail!(
                    "Cannot infer the repo from here. Pass `--repo <repo>` (or run from a session)."
                ),
            }
        }
    };

    if !config.repos.contains_key(&repo_name) {
        let names = config.all_names().join(", ");
        anyhow::bail!("Unknown repo: {repo_name}\nKnown: {names}");
    }

    let session_name = format!("{repo_name}/{campaign}");
    if tmux.session_exists(&session_name) {
        anyhow::bail!("{session_name} has a live session -- retire or kill it before archiving.");
    }

    let repo_dir = config.resolve_dir(&repo_name)?;
    let dest = primitives::archive_campaign(&config.layout, &repo_dir, campaign)?;
    eprintln!("Archived {repo_name}/{campaign} -> {}", dest.display());
    Ok(())
}

/// Migrate a repo's campaigns/ tree to the 2-level layout.
///
/// Resolves the repo dir from `--dir` (explicit, config-free) or the config
/// repo key, builds the plan, prints it, and applies it unless `--dry_run`.
pub(crate) fn cmd_migrate_layout(
    repo: Option<&str>,
    dir: Option<&std::path::Path>,
    dry_run: bool,
    keep_archives: bool,
) -> Result<()> {
    let repo_dir = match (dir, repo) {
        (Some(d), _) => d.to_path_buf(),
        (None, Some(r)) => {
            let config = Config::load()?;
            config.resolve_dir(r)?
        }
        (None, None) => {
            anyhow::bail!(
                "Specify a repo to migrate: `muxr migrate-layout <repo>` or `--dir <path>`."
            )
        }
    };

    let plan = migrate::plan(&repo_dir)?;
    let opts = migrate::Opts {
        dry_run,
        keep_archives,
    };
    migrate::print_plan(&repo_dir, &plan, &opts);
    migrate::execute(&repo_dir, &plan, &opts)?;
    Ok(())
}

/// Split a tmux session name into `(repo, campaign)`.
///
/// Session names are exactly two levels: `<repo>/<campaign>`. Campaigns are
/// validated kebab-case, so they never contain a slash; a name with more or
/// fewer than two non-empty components is not a campaign session (e.g. the
/// `muxr` control plane, or remote proxy names).
pub(crate) fn parse_session(name: &str) -> Option<(String, String)> {
    let (repo, campaign) = name.split_once('/')?;
    if repo.is_empty() || campaign.is_empty() || campaign.contains('/') {
        return None;
    }
    Some((repo.to_string(), campaign.to_string()))
}

/// Skip-serializing predicate for a borrowed, possibly-empty extension table:
/// serde hands `&field`, i.e. `&&toml::Table`, so the signature is double-ref.
fn ext_table_is_empty(t: &&toml::Table) -> bool {
    t.is_empty()
}

/// The launch intent handed to a resolver extension. Identifies WHAT muxr is
/// trying to launch; the extension answers WHERE/HOW (see `ResolveOutcome`).
#[derive(serde::Serialize)]
struct ResolveIntent<'a> {
    /// The full `<repo>/<campaign>` session name.
    session: &'a str,
    repo: &'a str,
    campaign: &'a str,
    /// The repo's open extension namespace (`[repos.<name>.ext]`), verbatim, so
    /// a resolver/launcher extension reads repo preference/hint data without a
    /// muxr schema change. Omitted when empty (keeps intents byte-compatible for
    /// repos that declare no `ext`).
    #[serde(skip_serializing_if = "ext_table_is_empty")]
    ext: &'a toml::Table,
    /// The repo checkout dir muxr resolved for this launch (the default for
    /// `ResolveOutcome.dir`). Handed over so a resolver needn't re-parse muxr's
    /// own config to find where the repo lives. Carries `config.resolve_dir`'s
    /// output: `~` is already expanded, so this is an absolute path string.
    repo_dir: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
}

/// A resolver extension's answer. Every field is optional: an omitted field
/// falls back to muxr's built-in `[layout]` computation, so an extension can
/// override just the parts it cares about (e.g. only `dir`).
#[derive(serde::Deserialize, Default)]
struct ResolveOutcome {
    /// Working directory for the session. Default: `config.resolve_dir(repo)`.
    dir: Option<String>,
    /// Campaign conventions file path. Default: the `[layout]` campaign path.
    campaign_md: Option<String>,
    /// Append-only log file path. Default: the `[layout]` log path.
    log_path: Option<String>,
    /// Runtime/tool name. Default: `config.resolve_tool(repo)`.
    runtime: Option<String>,
    /// Extra `--add-dir` paths, layered on top of the campaign's own paths.
    #[serde(default)]
    add_dirs: Vec<String>,
    /// Resume id override. Default: the resume id muxr was already given.
    resume_id: Option<String>,
}

/// The resolved layout facts `compose_launch_command` builds a launch from.
struct ResolvedLayout {
    dir: std::path::PathBuf,
    campaign_md: std::path::PathBuf,
    log_path: std::path::PathBuf,
    runtime: String,
    extra_add_dirs: Vec<String>,
    resume_id: Option<String>,
}

/// Resolve the layout facts for a launch. The built-in default reproduces the
/// 2.1 config-drive behavior exactly; when `[extensions].resolver` is set,
/// muxr invokes it (JSON intent in, JSON outcome out) and layers the returned
/// fields over those defaults. A resolver error is fatal (fail closed): once
/// you opt into deciding where a session launches, a silent fallback could
/// attach to the wrong campaign.
fn resolve_layout(
    config: &Config,
    repo_name: &str,
    campaign: &str,
    session_name: &str,
    resume_id: Option<&str>,
    model: Option<&str>,
) -> Result<ResolvedLayout> {
    let default_dir = config.resolve_dir(repo_name)?;
    let default_runtime = config.resolve_tool(repo_name, None);

    // Helper: derive the campaign/log path defaults from whatever dir won, so
    // an extension that overrides only `dir` relocates the whole layout
    // consistently rather than leaving the files pointed at the old root.
    let layout_paths = |dir: &std::path::Path| {
        (
            config.layout.campaign_md_path(dir, campaign),
            config.layout.log_md_path(dir, campaign),
        )
    };

    let Some(cmd) = config.extensions.resolver.as_deref() else {
        let (campaign_md, log_path) = layout_paths(&default_dir);
        return Ok(ResolvedLayout {
            dir: default_dir,
            campaign_md,
            log_path,
            runtime: default_runtime,
            extra_add_dirs: Vec::new(),
            resume_id: resume_id.map(str::to_string),
        });
    };

    let empty_ext = toml::Table::new();
    let ext = config
        .repos
        .get(repo_name)
        .map(|r| &r.ext)
        .unwrap_or(&empty_ext);
    let intent = ResolveIntent {
        session: session_name,
        repo: repo_name,
        campaign,
        ext,
        repo_dir: &default_dir.to_string_lossy(),
        resume_id,
        model,
    };
    let outcome: ResolveOutcome = crate::extension::invoke(cmd, "resolver", &intent)
        .with_context(|| format!("resolver extension for '{session_name}'"))?;

    let dir = outcome
        .dir
        .map(std::path::PathBuf::from)
        .unwrap_or(default_dir);
    let (default_campaign_md, default_log_path) = layout_paths(&dir);

    Ok(ResolvedLayout {
        campaign_md: outcome
            .campaign_md
            .map(std::path::PathBuf::from)
            .unwrap_or(default_campaign_md),
        log_path: outcome
            .log_path
            .map(std::path::PathBuf::from)
            .unwrap_or(default_log_path),
        dir,
        runtime: outcome.runtime.unwrap_or(default_runtime),
        extra_add_dirs: outcome.add_dirs,
        resume_id: outcome.resume_id.or_else(|| resume_id.map(str::to_string)),
    })
}

/// Materialise the full launch command for a session: the single source of
/// truth shared by `open`, `restore`, and `upgrade`.
///
/// Reconstructs the repo's launch settings, the composed HARNESS + campaign +
/// log system prompt (written to a temp file), and the campaign's `--add-dir`
/// paths, then asks the tool to assemble the command. The binary name is
/// resolved fresh from config each call, so a relaunch picks up a newly
/// installed harness version.
///
/// `resume_id` resumes a specific conversation; when it is `None` and
/// `continue_fallback` is set, the tool's `--continue` args are appended so a
/// restored session without a discovered id still re-attaches to its most
/// recent conversation. Returns the command and the session directory.
///
/// The campaign and log files are loaded best-effort; callers that might not
/// have them (restore/upgrade of an archived session) get a degraded
/// composition rather than a failure.
pub(crate) fn compose_launch_command(
    config: &Config,
    session_name: &str,
    resume_id: Option<&str>,
    model: Option<&str>,
    continue_fallback: bool,
) -> Result<(String, std::path::PathBuf)> {
    let (repo_name, campaign) = parse_session(session_name)
        .with_context(|| format!("cannot derive repo/campaign from '{session_name}'"))?;

    // The RESOLVER chokepoint (3.0): a launch intent in, the layout facts out.
    // The default reproduces the 2.1 config-drive layout exactly; a configured
    // `[extensions].resolver` may override any of dir/campaign_md/log_path/
    // runtime/resume_id and contribute extra --add-dirs.
    let resolved = resolve_layout(
        config,
        &repo_name,
        &campaign,
        session_name,
        resume_id,
        model,
    )?;
    let repo_dir = resolved.dir;
    let campaign_md = resolved.campaign_md;
    let log_path = resolved.log_path;
    let resume_id = resolved.resume_id.as_deref();

    let tool = resolved.runtime;
    let tool_config = config.tool_for(&tool);
    let repo = config.repos.get(&repo_name);

    // Start from the repo's existing launch settings; layer campaign
    // paths and the composed prompt on top.
    let mut settings = repo.map(|v| v.launch.clone()).unwrap_or_default();

    // Campaign and log files distinguish ABSENT from PRESENT-BUT-UNPARSEABLE.
    // A relaunch must keep the repo-level prompt and campaign --add-dir paths
    // even when the log/campaign file is MISSING -- e.g. an archived-but-still-
    // running session; that degrades cleanly (empty body / no campaign paths).
    // But a file that EXISTS and fails to parse (a one-character frontmatter
    // typo) must FAIL LOUD, not silently strip a live session's HARNESS rules
    // and working dirs on the next recycle/upgrade (issue #11) -- consistent
    // with the resolver extension's fail-closed contract. Genuinely fatal
    // errors (unknown repo, unparseable slug) are surfaced above.
    let (campaign_data, campaign_body) =
        primitives::load_optional(&campaign_md, primitives::load_campaign)
            .with_context(|| {
                format!(
                    "campaign file present but unparseable: {} \
                     (fix the frontmatter or remove the file)",
                    campaign_md.display()
                )
            })?
            .unwrap_or_default();
    // Only the entrypoint (the movable pointer) goes inline; the full log body
    // stays on disk and is pointed at, not snapshotted into the prompt.
    let entrypoint = primitives::load_optional(&log_path, primitives::load_log)
        .with_context(|| {
            format!(
                "log file present but unparseable: {} \
                 (fix the frontmatter or remove the file)",
                log_path.display()
            )
        })?
        .map(|(log, _)| log.entrypoint)
        .unwrap_or_default();

    let composed = primitives::compose_prompt(
        &campaign,
        &campaign_body,
        &entrypoint,
        &campaign_md,
        &log_path,
    );

    // Claude Code rejects --append-system-prompt and --append-system-prompt-file
    // together, and multi-line content via shell send-keys breaks parsing.
    // Resolve EVERY configured HARNESS.md-style prompt file, combine them with
    // the composed campaign+log prompt, write to a single temp file, and pass
    // only that one --append-system-prompt-file.
    //
    // Both the plural `append_system_prompt_files` (base + overlay, e.g.
    // HARNESS-base.md + a per-repo HARNESS.md) and the singular
    // `append_system_prompt_file` must be folded in here. The plural takes
    // precedence, mirroring launch_command_with_settings. Crucially we then
    // CLEAR the plural array below: otherwise launch_command_with_settings
    // would prefer the (un-composed) plural array and silently drop this
    // composed temp file -- which dropped the campaign + log entrypoint from
    // the system prompt for every repo configured with the array.
    let harness_files: Vec<String> = settings
        .append_system_prompt_files
        .clone()
        .or_else(|| settings.append_system_prompt_file.clone().map(|f| vec![f]))
        .unwrap_or_default();

    let mut harness_md_content = String::new();
    for file in &harness_files {
        let expanded = shellexpand::tilde(file).to_string();
        let path = if expanded.starts_with('/') {
            std::path::PathBuf::from(expanded)
        } else {
            repo_dir.join(&expanded)
        };
        // Fail LOUD if a CONFIGURED prompt file (append_system_prompt_file[s])
        // is missing or unreadable: silently skipping it composes a live session
        // with a truncated system prompt (no HARNESS rules), the exact silent
        // degrade #11 was about. A prompt file the operator named must exist.
        let text = primitives::read_text(&path)
            .with_context(|| format!("configured harness prompt file {file:?}"))?;
        if !harness_md_content.is_empty() {
            harness_md_content.push_str("\n\n");
        }
        harness_md_content.push_str(text.trim_end());
    }

    let full_prompt = if harness_md_content.trim().is_empty() {
        composed
    } else {
        format!("{}\n\n---\n\n{}", harness_md_content.trim_end(), composed)
    };

    let tmp_path = std::env::temp_dir().join(format!("muxr-prompt-{repo_name}-{campaign}.md"));
    std::fs::write(&tmp_path, &full_prompt)?;

    // Replace both prompt-file fields with our single composed temp file. The
    // plural array MUST be cleared so launch uses the composed file, not the
    // raw HARNESS files (which omit the campaign + log).
    settings.append_system_prompt = None;
    settings.append_system_prompt_files = None;
    settings.append_system_prompt_file = Some(tmp_path.to_string_lossy().to_string());

    for path in &campaign_data.paths {
        let expanded = primitives::expand_home(path);
        if !settings.add_dirs.iter().any(|d| d == &expanded) {
            settings.add_dirs.push(expanded);
        }
    }

    // Extra working dirs contributed by a resolver extension, on top of the
    // campaign's own paths. De-duped against whatever is already present.
    for path in &resolved.extra_add_dirs {
        let expanded = primitives::expand_home(path);
        if !settings.add_dirs.iter().any(|d| d == &expanded) {
            settings.add_dirs.push(expanded);
        }
    }

    let tool_cmd = match &tool_config {
        Some(h) => {
            let mut cmd =
                h.launch_command_with_settings(Some(session_name), resume_id, model, &settings)?;
            // No discovered id but a continue fallback was requested: re-attach
            // to the most recent conversation rather than starting cold.
            if resume_id.is_none() && continue_fallback {
                for arg in &h.continue_args {
                    cmd.push(' ');
                    cmd.push_str(arg);
                }
            }
            cmd
        }
        None => tool.clone(),
    };

    Ok((tool_cmd, repo_dir))
}

/// Open or attach to a remote proxy session: muxr lab bootc
fn cmd_open_remote(tmux: &Tmux, config: &Config, remote_name: &str, args: &[String]) -> Result<()> {
    let remote = config
        .remote(remote_name)
        .context("Remote not found in config")?;

    let context = if args.len() >= 2 {
        args[1..].join("/")
    } else {
        "default".to_string()
    };

    // "muxr lab ls" lists remote instances and their sessions
    if context == "ls" {
        return cmd_remote_ls(remote, remote_name);
    }

    let session = format!("{remote_name}/{context}");

    let instance = remote.instance_name(&context);

    if tmux.session_exists(&session) {
        eprintln!("Attaching to {session} (remote)");
        tmux.attach(&session)?;
    } else {
        // Bootstrap Claude config on first connect
        if let Err(e) = remote::bootstrap_claude_config(remote, &instance) {
            eprintln!("  Bootstrap warning: {e}");
        }

        let connect_cmd = remote::connect_command(remote, &instance, &context)?;
        eprintln!("Creating {session} -> {instance} via {}", remote.connect);
        let home = dirs::home_dir().context("No home directory")?;
        tmux.create_session(&session, &home, &connect_cmd, &[], None)?;
        tmux.attach(&session)?;
    }

    Ok(())
}

/// List running instances and their remote tmux sessions.
fn cmd_remote_ls(remote: &config::Remote, remote_name: &str) -> Result<()> {
    let instances = remote::list_instances(remote)?;
    if instances.is_empty() {
        println!("No running instances for {remote_name}");
        return Ok(());
    }

    for instance in &instances {
        println!("  {instance}:");
        match remote::list_remote_sessions(remote, instance) {
            Ok(sessions) if !sessions.is_empty() => {
                for sname in sessions {
                    println!("    {remote_name}/{sname}");
                }
            }
            _ => println!("    (no tmux sessions)"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_intent_serializes_resolved_repo_dir() {
        // The resolver is handed the repo's resolved checkout dir (the same
        // value muxr uses for the default `dir`), so it needn't re-parse muxr's
        // config to find where the repo lives. Additive: existing resolvers
        // that ignore `repo_dir` are unaffected.
        let empty = toml::Table::new();
        let intent = ResolveIntent {
            session: "work/auth-revamp",
            repo: "work",
            campaign: "auth-revamp",
            ext: &empty,
            repo_dir: "/Users/me/src/work",
            resume_id: None,
            model: None,
        };
        let json = serde_json::to_value(&intent).unwrap();
        assert_eq!(json["repo_dir"], "/Users/me/src/work");
        // Empty ext is omitted, so intents for repos with no `ext` stay
        // byte-compatible with pre-3.6 resolvers.
        assert!(json.get("ext").is_none());
        assert_eq!(json["repo"], "work");
        // resume_id/model stay omitted when absent (skip_serializing_if).
        assert!(json.get("resume_id").is_none());
        assert!(json.get("model").is_none());
    }
}
