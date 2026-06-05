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
        primitives::scaffold_switchboard(&dir)?;
        return cmd_open(tmux, &config, name, primitives::SWITCHBOARD, false);
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
    if !primitives::campaign_md_path(&repo_dir, campaign).is_file() {
        primitives::scaffold_campaign_stub(&repo_dir, campaign)?;
    }
    let campaign_md = primitives::campaign_file(&repo_dir, campaign)?;
    let log_path = primitives::resolve_or_scaffold_session(&repo_dir, campaign)?;

    let session_name = format!("{repo_name}/{campaign}");

    if tmux.session_exists(&session_name) {
        ui::action(&format!("attaching to {session_name}"));
        tmux.attach(&session_name)?;
        return Ok(());
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
        state::SavedState::session_id_for(&session_name)
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
        ui::detail("paths", &format!("+{} --add-dir", campaign_data.paths.len()));
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

    ui::action("launching…");
    tmux.create_session(&session_name, &session_dir, &tool_cmd)?;
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

    let new_md = primitives::scaffold_shard(&repo_dir, &hub, new)?;
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
    let campaign_md = primitives::campaign_md_path(&repo_dir, &campaign);
    let log_md = primitives::log_md_path(&repo_dir, &campaign);

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
/// The deliberate alternative to compact-looping. Before killing the session,
/// muxr asks the agent to flush its current state into `log.md` (set a tight
/// `entrypoint:` + append a dated log entry -- the procedure lives in the muxr
/// skill, so it never drifts from the layout), then `/exit`. muxr WAITS for the
/// agent to actually exit -- agent-paced, no wall-clock guess, since a flush can
/// take a long time -- then reopens the session FRESH (no resume) so it
/// rehydrates from that pointer. The prior conversation persists on disk and is
/// recoverable via --resume, so recycling never destroys context.
pub(crate) fn cmd_recycle(
    tmux: &Tmux,
    name: Option<&str>,
    no_serialize: bool,
    wait: u64,
) -> Result<()> {
    let config = Config::load()?;

    let session = session_or_current(tmux, name)?;
    let (repo_name, campaign) = parse_session(&session)
        .with_context(|| format!("'{session}' is not a <repo>/<campaign> session"))?;
    if !tmux.session_exists(&session) {
        anyhow::bail!("No live session named {session}.");
    }
    let repo_dir = config.resolve_dir(&repo_name)?;
    let log_md = primitives::log_md_path(&repo_dir, &campaign);
    // The campaign's declared work surface: the project repos this session
    // touches. A flush must reach all of these, not just log.md, or in-flight
    // work in the project repos is stranded when the session dies.
    let locales = primitives::campaign_md_path(&repo_dir, &campaign);
    let locales = primitives::load_campaign(&locales)
        .map(|(c, _)| c.paths)
        .unwrap_or_default();

    // Locate the harness process so we can wait for the agent's own exit as
    // the "flush complete" signal, rather than guessing a wall-clock delay.
    let tool = config.resolve_tool(&repo_name, None);
    let tool_def = config.tool_for(&tool);
    let bin = tool_def
        .as_ref()
        .map(|t| t.bin.clone())
        .unwrap_or_else(|| "claude".to_string());
    let harness_pid = tmux.pane_pid(&session).ok().flatten().and_then(|sp| {
        state::descendant_pids(sp)
            .into_iter()
            .find(|pid| state::pid_runs_bin(*pid, &bin))
    });

    if no_serialize {
        // Skip the flush; just exit (caller asserts state is already on disk).
        let exit_cmd = tool_def
            .as_ref()
            .and_then(|t| t.exit_command.clone())
            .unwrap_or_else(|| "/exit".to_string());
        ui::action(&format!("recycle {session}: exiting (no flush)"));
        tmux.send_text(&session, &exit_cmd)?;
        if let Some(pid) = harness_pid {
            tool::wait_for_exit(pid, 20);
        }
    } else {
        // Ask the agent to flush its state to the pointer, then exit. The
        // instruction is self-contained (names the exact log.md path), so it
        // doesn't depend on any external /serialize command being 2.0-aware.
        let locale_clause = if locales.is_empty() {
            String::new()
        } else {
            format!(
                " (2) For each project repo you've touched ({}): make sure in-flight work is \
                 captured -- commit it, or record the branch + uncommitted changes + next step \
                 (in the log entry) so nothing is stranded there.",
                locales.join(", ")
            )
        };
        let msg = format!(
            "[muxr recycle] Before this session is recycled, flush your state to ALL the locales \
             you've been working in so a fresh session resumes cleanly. (1) Update {} -- set the \
             `entrypoint:` frontmatter to a tight \"where we are / what's next\" line and append a \
             dated entry under `## Log` with current state and open threads.{} Then run /exit. \
             muxr is waiting for your exit and will reopen this session FRESH from that pointer. \
             Take as long as you need.",
            log_md.display(),
            locale_clause
        );
        ui::action(&format!(
            "recycle {session}: asked the agent to flush -> {} then exit",
            log_md.display()
        ));
        tmux.send_text(&session, &msg)?;
        ui::note("waiting for the agent to finish and exit (agent-paced)…");
        match harness_pid {
            Some(pid) => tool::wait_for_exit(pid, wait as u32),
            // No detectable harness process: fall back to a short grace.
            None => std::thread::sleep(std::time::Duration::from_secs(5)),
        }
    }

    // Clean slate, then recreate fresh from the (now-updated) pointer.
    if tmux.session_exists(&session) {
        tmux.kill_session(&session)?;
    }
    let (tool_cmd, session_dir) = compose_launch_command(&config, &session, None, None, false)?;
    config.run_pre_create_hooks(&session_dir);
    tmux.create_session(&session, &session_dir, &tool_cmd)?;

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
    let dest = primitives::archive_campaign(&repo_dir, campaign)?;
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
            anyhow::bail!("Specify a repo to migrate: `muxr migrate-layout <repo>` or `--dir <path>`.")
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

    let repo_dir = config.resolve_dir(&repo_name)?;
    let campaign_md = primitives::campaign_md_path(&repo_dir, &campaign);
    let log_path = primitives::log_md_path(&repo_dir, &campaign);

    let tool = config.resolve_tool(&repo_name, None);
    let tool_config = config.tool_for(&tool);
    let repo = config.repos.get(&repo_name);

    // Start from the repo's existing launch settings; layer campaign
    // paths and the composed prompt on top.
    let mut settings = repo.map(|v| v.launch.clone()).unwrap_or_default();

    // Campaign and log files are loaded best-effort. A relaunch must keep
    // the repo-level prompt and campaign --add-dir paths even when the log
    // body file is missing -- e.g. an archived-but-still-running session.
    // Without this an in-place upgrade of such a session would silently drop
    // its HARNESS rules and working dirs. Missing or unparseable files
    // degrade the composition (empty body / no campaign paths) instead of
    // failing the whole relaunch. Genuinely fatal errors (unknown repo,
    // unparseable slug) are still surfaced above, so callers keep their
    // name+resume fallback for the truly-unrecoverable case.
    let (campaign_data, campaign_body) = primitives::load_campaign(&campaign_md).unwrap_or_default();
    // Only the entrypoint (the movable pointer) goes inline; the full log body
    // stays on disk and is pointed at, not snapshotted into the prompt.
    let entrypoint = primitives::load_log(&log_path)
        .map(|(log, _)| log.entrypoint)
        .unwrap_or_default();

    let composed =
        primitives::compose_prompt(&campaign, &campaign_body, &entrypoint, &campaign_md, &log_path);

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
        if let Ok(text) = std::fs::read_to_string(&path) {
            if !harness_md_content.is_empty() {
                harness_md_content.push_str("\n\n");
            }
            harness_md_content.push_str(text.trim_end());
        }
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
        tmux.create_session(&session, &home, &connect_cmd)?;
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
