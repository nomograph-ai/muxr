use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_tool")]
    pub default_tool: String,
    pub repos: HashMap<String, Repo>,
    #[serde(default)]
    pub remotes: HashMap<String, Remote>,
    #[serde(default)]
    pub hooks: Hooks,
    #[serde(default)]
    pub tools: HashMap<String, Tool>,
    /// Filesystem layout of managed repos (campaigns dir, file names, reserved
    /// slugs). Omitted -> the built-in 2-level defaults. Makes the harness
    /// layout DATA, not compiled-in.
    #[serde(default)]
    pub layout: Layout,
    /// Subprocess extension points. Each is a command muxr invokes at an
    /// opinionated chokepoint (JSON in -> JSON out); absent -> the built-in
    /// default. See `extension.rs` for the contract.
    #[serde(default)]
    pub extensions: Extensions,
    /// Environment variables set on each campaign tmux session (`new-session
    /// -e KEY=VALUE`, tmux 3.2+). Values are templated with `{session}`,
    /// `{repo}`, `{campaign}`, and `{session_slug}` (the session name with any
    /// char outside `[A-Za-z0-9_-]` mapped to `-`). This is how a session gets
    /// coupled to an external tool generically -- e.g. binding synthesist with
    /// `SYNTHESIST_SESSION = "{session_slug}"` -- without muxr core knowing
    /// about that tool.
    #[serde(default)]
    pub session_env: std::collections::HashMap<String, String>,
    /// Interactive chooser. Absent -> muxr's built-in campaign-aware TUI (the
    /// default; knows about dormant campaigns, recycle/archive/rename). Set
    /// `command` to delegate selection to an external session picker (e.g.
    /// `sesh connect $(sesh list)`); that picker owns attach, and muxr's
    /// campaign lifecycle stays available via subcommands.
    #[serde(default)]
    pub chooser: Chooser,
}

/// External chooser delegation. The built-in TUI does far more than a generic
/// tmux picker (opens dormant campaigns, recycle/archive/rename); `command` is
/// a thin opt-out for users who prefer their own picker for plain attach, NOT
/// a full replacement.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Chooser {
    /// Shell command run (with an inherited terminal) instead of the built-in
    /// TUI. The command owns listing + attaching. Absent -> built-in.
    #[serde(default)]
    pub command: Option<String>,
}

/// The 3.0 extension contract: one subprocess mechanism for every fiddly bit
/// that keeps changing. Each field is a command (`sh -c <cmd>`) invoked with
/// structured JSON on stdin and structured JSON on stdout. An unset field
/// means muxr runs its built-in default and behaves exactly as 2.1 -- so a
/// config with no `[extensions]` is a fully usable launcher.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Extensions {
    /// RESOLVER: given a launch intent (`{session, repo, campaign, resume_id,
    /// model}`) return the layout facts (`{dir, campaign_md, log_path,
    /// runtime, add_dirs, resume_id}`); any omitted field falls back to the
    /// built-in `[layout]` computation. Absent -> the 2.1 config-drive layout.
    #[serde(default)]
    pub resolver: Option<String>,
    /// MAKE-DURABLE: fired before a session is recycled or closed. Receives
    /// `{session, repo, campaign, dir, campaign_md, log_path}` and supplies
    /// the agent-facing flush message (JSON `{message}`) -- or an empty
    /// message to skip. Absent -> the built-in recycle-flush prompt.
    #[serde(default)]
    pub make_durable: Option<String>,
}

/// Filesystem layout of muxr-managed repos. Defaults reproduce the built-in
/// 2-level model (`campaigns/<campaign>/{campaign.md,log.md}`); a repo can
/// override via `[layout]` so the harness layout is data, not compiled-in.
/// Path-construction methods are implemented in `primitives.rs`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Layout {
    /// Directory under a repo holding campaigns. Default `campaigns`.
    #[serde(default = "default_campaigns_dir")]
    pub campaigns_dir: String,
    /// Per-campaign conventions file. Default `campaign.md`.
    #[serde(default = "default_campaign_file")]
    pub campaign_file: String,
    /// Per-campaign append-only log file. Default `log.md`.
    #[serde(default = "default_log_file")]
    pub log_file: String,
    /// Reserved dir under the campaigns dir for archived campaigns. Default `archive`.
    #[serde(default = "default_archive_dir")]
    pub archive_dir: String,
    /// Reserved campaign slug for the repo switchboard. Default `switchboard`.
    #[serde(default = "default_switchboard_slug")]
    pub switchboard_slug: String,
}

fn default_campaigns_dir() -> String {
    "campaigns".to_string()
}
fn default_campaign_file() -> String {
    "campaign.md".to_string()
}
fn default_log_file() -> String {
    "log.md".to_string()
}
fn default_archive_dir() -> String {
    "archive".to_string()
}
fn default_switchboard_slug() -> String {
    "switchboard".to_string()
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            campaigns_dir: default_campaigns_dir(),
            campaign_file: default_campaign_file(),
            log_file: default_log_file(),
            archive_dir: default_archive_dir(),
            switchboard_slug: default_switchboard_slug(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Hooks {
    /// Commands to run before creating a new session.
    #[serde(default)]
    pub pre_create: Vec<String>,
    /// Extra PATH entries for hook commands. Supports ~ expansion.
    /// Prepended to the default system PATH.
    #[serde(default)]
    pub path: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Repo {
    pub dir: String,
    pub color: String,
    /// Override default_tool for this repo.
    #[serde(default)]
    pub tool: Option<String>,
    /// Tool-launch settings. Passed through to the runtime at session start.
    #[serde(default)]
    pub launch: LaunchSettings,
}

/// Settings passed to the tool on launch. Muxr passes these through
/// to the runtime -- it does not interpret them.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct LaunchSettings {
    /// Text appended to the system prompt. Multiple entries joined with newlines.
    #[serde(default)]
    pub append_system_prompt: Option<Vec<String>>,
    /// File to append to the system prompt (path supports ~).
    #[serde(default)]
    pub append_system_prompt_file: Option<String>,
    /// Multiple files to append to the system prompt, in order. Each is read
    /// and concatenated with `\n\n` before delivery. Takes precedence over
    /// `append_system_prompt_file` if both are set. Use for base + overlay
    /// prompt composition (e.g. shared HARNESS-base.md + harness-specific
    /// HARNESS.md).
    #[serde(default)]
    pub append_system_prompt_files: Option<Vec<String>>,
    /// Additional directories the harness can access.
    #[serde(default)]
    pub add_dirs: Vec<String>,
    /// Move cwd/git/env info out of system prompt for better cache hits.
    #[serde(default)]
    pub exclude_dynamic_prompt: bool,
    /// Per-harness wrapper override. When set, replaces the tool's
    /// `wrapper` for this harness only. Lets each harness point at its
    /// own nono profile without forcing one shared sandbox shape.
    /// Example: `nono run --profile dunn --` for the dunn harness.
    #[serde(default)]
    pub wrapper: Option<String>,
}

/// How to discover harness session IDs from running processes.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SessionDiscovery {
    /// Walk the process tree, look for a session file per PID.
    File {
        /// Path pattern with `{pid}` placeholder.
        pattern: String,
        /// JSON key containing the session ID.
        id_key: String,
    },
    /// No session discovery (tool doesn't support resume).
    None,
}

/// Configuration for a harness (AI coding tool).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tool {
    /// Binary name or path.
    pub bin: String,
    /// Args for initial launch. Supports `{name}` interpolation.
    #[serde(default)]
    pub args: Vec<String>,
    /// Args for resuming a session. Supports `{session_id}` interpolation.
    #[serde(default)]
    pub resume_args: Vec<String>,
    /// Args for setting the model. Supports `{model}` interpolation.
    #[serde(default)]
    pub model_args: Vec<String>,
    /// Command to send to the pane on rename. Supports `{name}` interpolation.
    #[serde(default)]
    pub rename_command: Option<String>,
    /// Command to send for live model switch. Supports `{model}` interpolation.
    #[serde(default)]
    pub model_switch_command: Option<String>,
    /// Command to compact context.
    #[serde(default)]
    pub compact_command: Option<String>,
    /// Command to exit the harness gracefully.
    #[serde(default)]
    pub exit_command: Option<String>,
    /// Args to pass when session ID is missing (fallback resume).
    #[serde(default)]
    pub continue_args: Vec<String>,
    /// Args for forking a session (new UUID from existing conversation).
    #[serde(default)]
    pub fork_args: Vec<String>,
    /// How to discover session IDs.
    #[serde(default = "default_discovery_none")]
    pub session_discovery: SessionDiscovery,
    /// Optional command prefix prepended to the launch command.
    /// Example: `"nono run --profile ~/.config/nono/profiles/pi --"`.
    /// The wrapper is inserted ahead of `bin` so the resulting command
    /// becomes `<wrapper> <bin> <args...>`.
    #[serde(default)]
    pub wrapper: Option<String>,
    /// How to deliver the appended system prompt to the tool.
    /// `File` (default) passes `--append-system-prompt-file <path>` (Claude Code).
    /// `String` reads the file and passes `--append-system-prompt <content>` (Pi).
    #[serde(default)]
    pub prompt_mode: PromptMode,
    /// Whether this runtime accepts `--add-dir <path>` for extra working dirs.
    /// `None` inherits the built-in (Claude: yes; Pi: no -- sandboxing is
    /// external). A runtime adapter sets this instead of muxr branching on the
    /// bin name, so adding a runtime stays pure config.
    #[serde(default)]
    pub supports_add_dirs: Option<bool>,
}

impl Tool {
    /// Whether `--add-dir` should be emitted for this runtime. Defaults to
    /// true (most CLIs accept it); a runtime opts out via `supports_add_dirs`.
    fn emits_add_dirs(&self) -> bool {
        self.supports_add_dirs.unwrap_or(true)
    }
}

/// How a tool consumes the appended system prompt.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PromptMode {
    /// Pass `--append-system-prompt-file <path>`. Default; matches Claude Code.
    #[default]
    File,
    /// Read the file and pass `--append-system-prompt <content>`. Used by Pi.
    String,
}

fn default_discovery_none() -> SessionDiscovery {
    SessionDiscovery::None
}

/// Overlay user-supplied fields on top of a built-in tool definition.
///
/// Each field is checked against its type-default. If the user's value
/// equals the type-default, it's treated as "not set" and the built-in's
/// value wins. Otherwise the user's value wins.
///
/// `bin` is always taken from the user (they explicitly named the tool;
/// if they want a different bin path that's the whole point of overriding).
/// `prompt_mode` defaults to PromptMode::File at the type level, so an
/// unspecified user override would silently revert Pi to File mode --
/// here we let the user value win even when it equals the default,
/// matching the principle that `prompt_mode` is the single most-likely
/// override field.
fn merge_tool_with_builtin(user: Tool, builtin: Tool) -> Tool {
    Tool {
        bin: user.bin,
        args: if user.args.is_empty() {
            builtin.args
        } else {
            user.args
        },
        resume_args: if user.resume_args.is_empty() {
            builtin.resume_args
        } else {
            user.resume_args
        },
        model_args: if user.model_args.is_empty() {
            builtin.model_args
        } else {
            user.model_args
        },
        rename_command: user.rename_command.or(builtin.rename_command),
        model_switch_command: user.model_switch_command.or(builtin.model_switch_command),
        compact_command: user.compact_command.or(builtin.compact_command),
        exit_command: user.exit_command.or(builtin.exit_command),
        continue_args: if user.continue_args.is_empty() {
            builtin.continue_args
        } else {
            user.continue_args
        },
        fork_args: if user.fork_args.is_empty() {
            builtin.fork_args
        } else {
            user.fork_args
        },
        session_discovery: match user.session_discovery {
            SessionDiscovery::None => builtin.session_discovery,
            other => other,
        },
        wrapper: user.wrapper.or(builtin.wrapper),
        // prompt_mode is the single most common override and PromptMode::File is
        // also the type-default. Users who set `prompt_mode = "file"` explicitly
        // would be indistinguishable from the default; treat user value as
        // authoritative whenever they configured the tool at all.
        prompt_mode: user.prompt_mode,
        // None on the user side inherits the built-in's add-dir capability;
        // an explicit Some overrides it.
        supports_add_dirs: user.supports_add_dirs.or(builtin.supports_add_dirs),
    }
}

/// Reserved command names that cannot be used as repo names.
const RESERVED_NAMES: &[&str] = &[
    "init",
    "ls",
    "save",
    "restore",
    "new",
    "rename",
    "kill",
    "switch",
    "upgrade",
    "retire",
    "broadcast",
    "skill",
    "shard",
    "reorient",
    "recycle",
    "archive",
    "migrate-layout",
    "tmux-status",
    "completions",
];

impl Tool {
    /// Built-in Claude Code harness definition.
    pub fn builtin_claude() -> Self {
        Self {
            bin: "claude".to_string(),
            args: vec!["--name".to_string(), "{name}".to_string()],
            resume_args: vec!["--resume".to_string(), "{session_id}".to_string()],
            model_args: vec!["--model".to_string(), "{model}".to_string()],
            rename_command: Some("/rename {name}".to_string()),
            model_switch_command: Some("/model {model}".to_string()),
            compact_command: Some("/compact".to_string()),
            exit_command: Some("/exit".to_string()),
            continue_args: vec!["--continue".to_string()],
            fork_args: vec!["--fork-session".to_string()],
            session_discovery: SessionDiscovery::File {
                pattern: "~/.claude/sessions/{pid}.json".to_string(),
                id_key: "sessionId".to_string(),
            },
            wrapper: None,
            prompt_mode: PromptMode::File,
            supports_add_dirs: Some(true),
        }
    }

    /// Built-in Pi harness definition.
    ///
    /// Pi reads `.pi/settings.json` from cwd, so launch args stay empty;
    /// the model is selected via `--model {model}` like Claude.
    /// Session discovery references `~/.pi/sessions/{pid}.json`, which is
    /// written by an external Pi extension (not Pi itself).
    /// `wrapper` defaults to None and is overridable in user config (e.g.
    /// to wrap launch in a sandboxing tool).
    pub fn builtin_pi() -> Self {
        Self {
            bin: "pi".to_string(),
            args: vec![],
            resume_args: vec!["--resume".to_string(), "{session_id}".to_string()],
            model_args: vec!["--model".to_string(), "{model}".to_string()],
            rename_command: Some("/name {name}".to_string()),
            model_switch_command: Some("/model {model}".to_string()),
            compact_command: Some("/compact".to_string()),
            exit_command: Some("/quit".to_string()),
            continue_args: vec!["--continue".to_string()],
            fork_args: vec!["--fork".to_string(), "{session_id}".to_string()],
            session_discovery: SessionDiscovery::File {
                pattern: "~/.pi/sessions/{pid}.json".to_string(),
                id_key: "sessionId".to_string(),
            },
            wrapper: None,
            prompt_mode: PromptMode::String,
            supports_add_dirs: Some(false),
        }
    }

    /// Build the launch command with template interpolation.
    /// All interpolated values are shell-escaped.
    pub fn launch_command(
        &self,
        session_name: Option<&str>,
        resume_id: Option<&str>,
        model: Option<&str>,
    ) -> String {
        let mut parts = vec![self.bin.clone()];

        if let Some(name) = session_name {
            for arg in &self.args {
                parts.push(interpolate(arg, "name", name));
            }
        }

        if let Some(id) = resume_id {
            for arg in &self.resume_args {
                parts.push(interpolate(arg, "session_id", id));
            }
        }

        if let Some(m) = model {
            for arg in &self.model_args {
                parts.push(interpolate(arg, "model", m));
            }
        }

        parts.join(" ")
    }

    /// Build the resume command for restore. Uses --continue as fallback
    /// when session ID is lost.
    pub fn restore_command(&self, session_name: Option<&str>, resume_id: Option<&str>) -> String {
        if resume_id.is_some() {
            return self.launch_command(session_name, resume_id, None);
        }
        // No session ID -- fall back to --continue
        let mut parts = vec![self.bin.clone()];
        if let Some(name) = session_name {
            for arg in &self.args {
                parts.push(interpolate(arg, "name", name));
            }
        }
        for arg in &self.continue_args {
            parts.push(arg.clone());
        }
        parts.join(" ")
    }

    /// Build the launch command with harness-specific settings from the harness.
    ///
    /// If the tool has a `wrapper` set, the final command is
    /// `<wrapper> <launch_command> <settings flags...>`.
    /// `prompt_mode` controls how `append_system_prompt_file` is delivered:
    /// File (default) passes `--append-system-prompt-file <path>` (Claude),
    /// String reads the file contents and inlines them into
    /// `--append-system-prompt <content>` (Pi, which lacks a file variant).
    /// Pi additionally has no `--add-dir` flag, so `add_dirs` are skipped
    /// when `bin == "pi"`.
    pub fn launch_command_with_settings(
        &self,
        session_name: Option<&str>,
        resume_id: Option<&str>,
        model: Option<&str>,
        settings: &LaunchSettings,
    ) -> Result<String> {
        let mut cmd = self.launch_command(session_name, resume_id, model);

        if let Some(ref prompts) = settings.append_system_prompt {
            let joined = prompts.join("\n");
            cmd.push_str(&format!(
                " --append-system-prompt {}",
                shell_escape(&joined)
            ));
        }
        // Determine which file-based prompt source to use. The array field
        // takes precedence over the singular field when both are set.
        let effective_files: Option<Vec<String>> =
            if let Some(ref files) = settings.append_system_prompt_files {
                if settings.append_system_prompt_file.is_some() {
                    eprintln!(
                        "muxr warning: both append_system_prompt_files and \
                         append_system_prompt_file are set; using the array and \
                         ignoring the singular field"
                    );
                }
                Some(files.clone())
            } else {
                settings
                    .append_system_prompt_file
                    .as_ref()
                    .map(|f| vec![f.clone()])
            };

        if let Some(files) = effective_files {
            // Expand ~ / absolute paths; leave relative paths as-is so they
            // resolve from the harness's cwd at launch time.
            let expanded_paths: Vec<String> = files
                .iter()
                .map(|f| {
                    if f.starts_with('/') || f.starts_with('~') {
                        shellexpand::tilde(f).to_string()
                    } else {
                        f.clone()
                    }
                })
                .collect();

            match self.prompt_mode {
                PromptMode::File => {
                    if expanded_paths.len() == 1 {
                        // Single file -- pass directly to avoid temp-file churn.
                        cmd.push_str(&format!(
                            " --append-system-prompt-file {}",
                            shell_escape(&expanded_paths[0])
                        ));
                    } else {
                        // Multiple files -- compose into a temp file. Claude Code
                        // (and similar) only accept a single --append-system-prompt-file
                        // flag, so we materialise the composition.
                        let composed = read_and_join(&expanded_paths, &self.bin)?;
                        let tmp_path = write_composed_prompt(&composed)?;
                        cmd.push_str(&format!(
                            " --append-system-prompt-file {}",
                            shell_escape(&tmp_path)
                        ));
                    }
                }
                PromptMode::String => {
                    // Pi has no file variant. Read every file and inline the
                    // composition. Fail loud on read error -- a missing prompt
                    // file silently strips harness directives, which is worse
                    // than refusing to launch.
                    let composed = read_and_join(&expanded_paths, &self.bin)?;
                    cmd.push_str(&format!(
                        " --append-system-prompt {}",
                        shell_escape(&composed)
                    ));
                }
            }
        }
        // Runtimes that don't accept --add-dir (e.g. Pi -- sandboxing is
        // external via nono) opt out via `supports_add_dirs`; the rest
        // (claude) keep getting --add-dir as today. No bin-name branching.
        if self.emits_add_dirs() {
            for dir in &settings.add_dirs {
                let expanded = shellexpand::tilde(dir);
                cmd.push_str(&format!(" --add-dir {}", shell_escape(&expanded)));
            }
        }
        if settings.exclude_dynamic_prompt {
            cmd.push_str(" --exclude-dynamic-system-prompt-sections");
        }

        // Prepend the wrapper last so the rest of the command lines up
        // behind it: `<wrapper> <bin> <args...>`. The per-harness
        // settings.wrapper takes precedence over the tool default so
        // each harness can point at its own nono profile.
        let wrap = settings.wrapper.as_deref().or(self.wrapper.as_deref());
        if let Some(w) = wrap {
            cmd = format!("{} {}", w.trim(), cmd);
        }

        Ok(cmd)
    }

    /// Build the rename command to send to the pane.
    /// Uses raw interpolation -- this is a slash command sent as keystrokes,
    /// not a shell command.
    pub fn build_rename_command(&self, name: &str) -> Option<String> {
        self.rename_command
            .as_ref()
            .map(|cmd| interpolate_raw(cmd, "name", name))
    }
}

/// Interpolate a `{key}` placeholder with a shell-escaped value.
/// Use for values that will be parsed by a shell (launch commands).
pub fn interpolate(template: &str, key: &str, value: &str) -> String {
    let placeholder = format!("{{{key}}}");
    if template.contains(&placeholder) {
        template.replace(&placeholder, &shell_escape(value))
    } else {
        template.to_string()
    }
}

/// Interpolate a `{key}` placeholder with the raw value (no escaping).
/// Use for slash commands sent as keystrokes to a running harness --
/// the harness reads the literal characters, not a shell.
pub fn interpolate_raw(template: &str, key: &str, value: &str) -> String {
    let placeholder = format!("{{{key}}}");
    template.replace(&placeholder, value)
}

/// Read a list of file paths and join their contents with `\n\n`.
fn read_and_join(paths: &[String], bin: &str) -> Result<String> {
    let mut parts = Vec::with_capacity(paths.len());
    for path in paths {
        let content = std::fs::read_to_string(path).with_context(|| {
            format!("failed to read system-prompt file at {path} for tool {bin}")
        })?;
        parts.push(content);
    }
    Ok(parts.join("\n\n"))
}

/// Write composed prompt content to a temp file and return its path.
/// The temp file is created in $TMPDIR with a deterministic-ish prefix
/// so it can be inspected after launch if debugging is needed.
fn write_composed_prompt(content: &str) -> Result<String> {
    use std::io::Write;
    let mut path = std::env::temp_dir();
    // Use a fixed name; muxr runs single-threaded at launch time so
    // concurrent overwrites are not a concern.
    path.push("muxr-composed-system-prompt.md");
    let mut f = std::fs::File::create(&path)
        .with_context(|| format!("failed to create temp prompt file at {}", path.display()))?;
    f.write_all(content.as_bytes())
        .with_context(|| format!("failed to write temp prompt file at {}", path.display()))?;
    Ok(path.to_string_lossy().to_string())
}

/// Shell-escape a value by wrapping in single quotes.
fn shell_escape(s: &str) -> String {
    if s.contains('\'') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        format!("'{s}'")
    }
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

/// Resolve the config path from an optional `MUXR_CONFIG` override and the
/// home dir. Split out from `Config::path` so it is testable without
/// mutating process env. An empty override is ignored (falls back to home).
fn resolve_config_path(env_override: Option<String>, home: &std::path::Path) -> PathBuf {
    match env_override {
        Some(p) if !p.is_empty() => PathBuf::from(shellexpand::tilde(&p).to_string()),
        _ => home.join(".config").join("muxr").join("config.toml"),
    }
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

        // Validate no name collisions between repos, remotes, and tools
        for name in config.remotes.keys() {
            if config.repos.contains_key(name) {
                anyhow::bail!("Name collision: '{name}' is defined as both a repo and a remote");
            }
        }
        for name in config.tools.keys() {
            if config.repos.contains_key(name) {
                anyhow::bail!("Name collision: '{name}' is defined as both a tool and a repo");
            }
            if config.remotes.contains_key(name) {
                anyhow::bail!("Name collision: '{name}' is defined as both a remote and a repo");
            }
            if RESERVED_NAMES.contains(&name.as_str()) {
                anyhow::bail!("Repo name '{name}' is reserved (conflicts with built-in command)");
            }
        }

        Ok(config)
    }

    /// Resolve the config file path. `MUXR_CONFIG`, when set and non-empty,
    /// overrides the default `~/.config/muxr/config.toml`. The override lets
    /// tests and jig fixtures point muxr at an isolated config without
    /// hijacking `$HOME`. `~` in the override is expanded.
    pub fn path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        Ok(resolve_config_path(
            std::env::var("MUXR_CONFIG").ok(),
            &home,
        ))
    }

    /// State lives beside the config, so `MUXR_CONFIG` isolates both with a
    /// single override: `state.json` is always the config file's sibling.
    pub fn state_path() -> Result<PathBuf> {
        let cfg = Self::path()?;
        let dir = cfg
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(dir.join("state.json"))
    }

    pub fn resolve_dir(&self, repo: &str) -> Result<PathBuf> {
        let v = self
            .repos
            .get(repo)
            .with_context(|| format!("Unknown repo: {repo}"))?;
        let expanded = shellexpand::tilde(&v.dir);
        Ok(PathBuf::from(expanded.as_ref()))
    }

    /// All known names (repos + remotes) for validation and completions.
    pub fn all_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .repos
            .keys()
            .chain(self.remotes.keys())
            .map(|s| s.as_str())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Resolve which tool to use for a repo.
    /// Priority: explicit override > repo config > default_tool
    pub fn resolve_tool(&self, repo: &str, tool_override: Option<&str>) -> String {
        if let Some(t) = tool_override {
            return t.to_string();
        }
        if let Some(v) = self.repos.get(repo)
            && let Some(ref t) = v.tool
        {
            return t.clone();
        }
        self.default_tool.clone()
    }

    /// Resolve the templated `[session_env]` map for a concrete session name
    /// into the literal `KEY=VALUE` pairs to set on its tmux session. Tokens
    /// `{session}`, `{repo}`, `{campaign}`, `{session_slug}` are interpolated;
    /// the slug maps any char outside `[A-Za-z0-9_-]` to `-` (path-safe for
    /// tools like synthesist that reject `/` in a session segment).
    pub fn session_env_for(&self, session_name: &str) -> Vec<(String, String)> {
        if self.session_env.is_empty() {
            return Vec::new();
        }
        let (repo, campaign) = session_name
            .split_once('/')
            .unwrap_or((session_name, ""));
        let slug: String = session_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        self.session_env
            .iter()
            .map(|(k, v)| {
                let value = v
                    .replace("{session_slug}", &slug)
                    .replace("{session}", session_name)
                    .replace("{repo}", repo)
                    .replace("{campaign}", campaign);
                (k.clone(), value)
            })
            .collect()
    }

    /// Get the harness config for a tool name.
    ///
    /// User config in `[tools.<name>]` is treated as a PARTIAL override over
    /// the built-in definition for known tools (claude, pi). Fields the user
    /// did not specify keep their built-in defaults rather than collapsing
    /// to their type-default. This was the cause of muxr save returning
    /// null sessionIds: the user's `[tools.pi]` block specified only `bin`
    /// and `prompt_mode`, which under full-replace semantics wiped
    /// `session_discovery`, `resume_args`, etc.
    ///
    /// Heuristic: a field is considered "unset by the user" if it equals
    /// its type default (empty Vec, None Option, SessionDiscovery::None).
    /// Users who deliberately want to clear a field cannot do so; the
    /// trade-off is acceptable because clearing a builtin's resume-args or
    /// session-discovery rarely makes sense.
    pub fn tool_for(&self, tool: &str) -> Option<Tool> {
        let builtin = match tool {
            "claude" => Some(Tool::builtin_claude()),
            "pi" => Some(Tool::builtin_pi()),
            _ => None,
        };
        match (self.tools.get(tool).cloned(), builtin) {
            (Some(user), Some(builtin)) => Some(merge_tool_with_builtin(user, builtin)),
            (Some(user), None) => Some(user),
            (None, Some(builtin)) => Some(builtin),
            (None, None) => None,
        }
    }

    /// All configured harness names (explicit + built-in).
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        // Add built-in claude if not overridden
        if !names.contains(&"claude".to_string()) {
            names.push("claude".to_string());
        }
        if !names.contains(&"pi".to_string()) {
            names.push("pi".to_string());
        }
        names.sort();
        names
    }

    pub fn is_remote(&self, name: &str) -> bool {
        self.remotes.contains_key(name)
    }

    pub fn remote(&self, name: &str) -> Option<&Remote> {
        self.remotes.get(name)
    }

    /// Run pre_create hooks in a directory. Hooks run with the shims PATH
    /// so mise-managed tools are available. Failures are warnings, not fatal.
    pub fn run_pre_create_hooks(&self, dir: &std::path::Path) {
        if self.hooks.pre_create.is_empty() {
            return;
        }
        let path = self.hooks_path();
        for cmd in &self.hooks.pre_create {
            // Capture output so a hook's raw stdout (kit/rune sync) doesn't
            // dump into the launch. Show a transient "running" line first so a
            // slow sync reads as progress, not a hang; the ok/warn result
            // overwrites it. Reveal the captured output only when the hook
            // fails.
            crate::ui::step_start(&format!("setup: {cmd}"));
            let result = std::process::Command::new("sh")
                .args(["-c", cmd])
                .current_dir(dir)
                .env("PATH", &path)
                .output();
            match result {
                Ok(o) if o.status.success() => crate::ui::ok(&format!("setup: {cmd}")),
                Ok(o) => {
                    crate::ui::warn(&format!("setup: {cmd} exited {}", o.status));
                    let out = String::from_utf8_lossy(&o.stdout);
                    let err = String::from_utf8_lossy(&o.stderr);
                    for line in out.lines().chain(err.lines()) {
                        eprintln!("      {line}");
                    }
                }
                Err(e) => crate::ui::warn(&format!("setup: {cmd} failed: {e}")),
            }
        }
    }

    /// Build PATH for hook execution. Uses configured paths if set,
    /// otherwise falls back to system PATH.
    fn hooks_path(&self) -> String {
        let system = "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
        if self.hooks.path.is_empty() {
            // Inherit current PATH, fall back to system
            std::env::var("PATH").unwrap_or_else(|_| system.to_string())
        } else {
            let expanded: Vec<String> = self
                .hooks
                .path
                .iter()
                .map(|p| shellexpand::tilde(p).to_string())
                .collect();
            format!("{}:{}", expanded.join(":"), system)
        }
    }

    pub fn color_for(&self, name: &str) -> &str {
        self.repos
            .get(name)
            .map(|v| v.color.as_str())
            .or_else(|| self.remotes.get(name).map(|r| r.color.as_str()))
            .unwrap_or("#8a7f83")
    }

    /// Generate a default config file with example repos.
    pub fn default_template() -> String {
        r##"# muxr configuration
# Repos are named project estates. Each maps to a directory and a
# status-bar color. Sessions launch under `campaigns/<campaign>/` inside
# the repo directory, named `<repo>/<campaign>`.

default_tool = "claude"

# [repos.work]
# dir = "~/projects/work"
# color = "#7aa2f7"
# tool = "claude"    # optional, overrides default_tool
#
# [repos.work.launch]
# append_system_prompt_file = "HARNESS.md"
# add_dirs = ["~/docs/shared"]
#
# [repos.personal]
# dir = "~/projects/personal"
# color = "#9ece6a"

# [hooks]
# pre_create = ["mise install"]
# path = ["~/.local/share/mise/shims"]

# Tool definitions. Claude is built-in (zero config needed).
# Only define [tools.claude] to override the built-in defaults.
# Other tools must be configured explicitly.
#
# [tools.opencode]
# bin = "opencode"
# session_discovery = { type = "none" }
# supports_add_dirs = false   # runtime has no --add-dir (sandbox is external)

# Extensions (3.0): one subprocess contract for the fiddly bits. Each is a
# command run with JSON on stdin -> JSON on stdout; absent -> built-in default.
#
# [extensions]
# resolver = "my-resolver"        # layout decision; default = the [layout] above
# make_durable = "my-flush"       # recycle-flush message; default = built-in prompt

# Per-session tmux env (new-session -e). Templated with {session} {repo}
# {campaign} {session_slug}. Couples a session to an external tool generically.
#
# [session_env]
# SYNTHESIST_SESSION = "{session_slug}"

# Delegate the interactive picker to an external tool (default = built-in TUI).
#
# [chooser]
# command = "sesh connect \"$(sesh list | fzf)\""
"##
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_env_interpolates_tokens_and_slug() {
        let config: Config = toml::from_str(
            "[repos]\n\n[session_env]\nSYNTHESIST_SESSION = \"{session_slug}\"\nINFO = \"{repo}:{campaign}\"\n",
        )
        .unwrap();
        let mut env = config.session_env_for("work/in-place/fix");
        env.sort();
        assert_eq!(
            env,
            vec![
                ("INFO".to_string(), "work:in-place/fix".to_string()),
                (
                    "SYNTHESIST_SESSION".to_string(),
                    // slug maps the two slashes to dashes
                    "work-in-place-fix".to_string()
                ),
            ]
        );
    }

    #[test]
    fn session_env_empty_when_unconfigured() {
        let config: Config = toml::from_str("[repos]").unwrap();
        assert!(config.session_env_for("work/x").is_empty());
    }

    fn sample_config() -> Config {
        let toml_str = r##"
default_tool = "claude"

[repos.work]
dir = "~/projects/work"
color = "#7aa2f7"
tool = "claude"

[repos.personal]
dir = "~/projects/personal"
color = "#9ece6a"
tool = "opencode"

[remotes.lab]
project = "my-project"
zone = "us-central1-a"
user = "deploy"
color = "#d29922"

[tools.opencode]
bin = "opencode"
session_discovery = { type = "none" }
"##;
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn parse_valid_config() {
        let config = sample_config();
        assert_eq!(config.default_tool, "claude");
        assert_eq!(config.repos.len(), 2);
        assert_eq!(config.remotes.len(), 1);
        assert_eq!(config.tools.len(), 1);
    }

    #[test]
    fn default_tool_is_claude() {
        let config: Config = toml::from_str("[repos]").unwrap();
        assert_eq!(config.default_tool, "claude");
        assert!(config.tools.is_empty());
    }

    #[test]
    fn default_connect_is_mosh() {
        let config = sample_config();
        let lab = config.remotes.get("lab").unwrap();
        assert_eq!(lab.connect, "mosh");
    }

    #[test]
    fn all_names_sorted_and_deduped() {
        let config = sample_config();
        let names = config.all_names();
        assert_eq!(names, vec!["lab", "personal", "work"]);
    }

    #[test]
    fn is_remote_distinguishes() {
        let config = sample_config();
        assert!(config.is_remote("lab"));
        assert!(!config.is_remote("work"));
        assert!(!config.is_remote("nonexistent"));
    }

    #[test]
    fn color_for_harness() {
        let config = sample_config();
        assert_eq!(config.color_for("work"), "#7aa2f7");
    }

    #[test]
    fn color_for_remote() {
        let config = sample_config();
        assert_eq!(config.color_for("lab"), "#d29922");
    }

    #[test]
    fn color_for_unknown_returns_default() {
        let config = sample_config();
        assert_eq!(config.color_for("nonexistent"), "#8a7f83");
    }

    #[test]
    fn instance_name_simple() {
        let remote = Remote {
            project: "p".into(),
            zone: "z".into(),
            user: "u".into(),
            color: "#fff".into(),
            connect: "mosh".into(),
            instance_prefix: None,
        };
        assert_eq!(remote.instance_name("bootc"), "bootc");
    }

    #[test]
    fn instance_name_with_prefix() {
        let remote = Remote {
            project: "p".into(),
            zone: "z".into(),
            user: "u".into(),
            color: "#fff".into(),
            connect: "mosh".into(),
            instance_prefix: Some("lab-".into()),
        };
        assert_eq!(remote.instance_name("bootc"), "lab-bootc");
    }

    #[test]
    fn instance_name_replaces_slashes() {
        let remote = Remote {
            project: "p".into(),
            zone: "z".into(),
            user: "u".into(),
            color: "#fff".into(),
            connect: "mosh".into(),
            instance_prefix: None,
        };
        assert_eq!(remote.instance_name("api/auth"), "api-auth");
    }

    #[test]
    fn name_collision_harness_remote_rejected() {
        let toml_str = r##"
[repos.lab]
dir = "~/lab"
color = "#fff"

[remotes.lab]
project = "p"
zone = "z"
user = "u"
color = "#fff"
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let has_collision = config
            .remotes
            .keys()
            .any(|name| config.repos.contains_key(name));
        assert!(has_collision);
    }

    #[test]
    fn name_collision_tool_harness_detected() {
        let toml_str = r##"
[repos.opencode]
dir = "~/oc"
color = "#fff"

[tools.opencode]
bin = "opencode"
session_discovery = { type = "none" }
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let has_collision = config
            .tools
            .keys()
            .any(|name| config.repos.contains_key(name));
        assert!(has_collision);
    }

    #[test]
    fn reserved_harness_name_detected() {
        assert!(RESERVED_NAMES.contains(&"save"));
        assert!(RESERVED_NAMES.contains(&"switch"));
        assert!(RESERVED_NAMES.contains(&"upgrade"));
        assert!(RESERVED_NAMES.contains(&"retire"));
        assert!(RESERVED_NAMES.contains(&"broadcast"));
        assert!(!RESERVED_NAMES.contains(&"claude"));
    }

    #[test]
    fn config_path_honors_muxr_config_override() {
        let home = std::path::Path::new("/home/u");
        // No override -> default under home.
        assert_eq!(
            resolve_config_path(None, home),
            home.join(".config/muxr/config.toml")
        );
        // Empty override is ignored.
        assert_eq!(
            resolve_config_path(Some(String::new()), home),
            home.join(".config/muxr/config.toml")
        );
        // Non-empty override wins verbatim (absolute path).
        assert_eq!(
            resolve_config_path(Some("/tmp/fix/config.toml".to_string()), home),
            PathBuf::from("/tmp/fix/config.toml")
        );
    }

    #[test]
    fn hooks_default_empty() {
        let config: Config = toml::from_str("[repos]").unwrap();
        assert!(config.hooks.pre_create.is_empty());
        assert!(config.hooks.path.is_empty());
    }

    #[test]
    fn default_template_contains_default_tool() {
        let template = Config::default_template();
        assert!(template.contains("default_tool = \"claude\""));
    }

    // -- Harness config tests --

    #[test]
    fn builtin_claude_harness() {
        let h = Tool::builtin_claude();
        assert_eq!(h.bin, "claude");
        assert_eq!(h.rename_command, Some("/rename {name}".to_string()));
        assert!(matches!(h.session_discovery, SessionDiscovery::File { .. }));
    }

    #[test]
    fn tool_for_returns_builtin_claude() {
        let config: Config = toml::from_str("[repos]").unwrap();
        let h = config.tool_for("claude").unwrap();
        assert_eq!(h.bin, "claude");
    }

    #[test]
    fn tool_for_returns_configured() {
        let config = sample_config();
        let h = config.tool_for("opencode").unwrap();
        assert_eq!(h.bin, "opencode");
    }

    #[test]
    fn tool_for_unknown_returns_none() {
        let config = sample_config();
        assert!(config.tool_for("cursor").is_none());
    }

    #[test]
    fn harness_config_partially_overrides_builtin() {
        // User config in [tools.<builtin-name>] is now treated as a
        // partial override on top of the builtin definition. Fields
        // the user did not specify keep their builtin values rather
        // than collapsing to type-defaults. This was the cause of
        // muxr save returning null sessionIds when [tools.pi] only
        // declared bin + prompt_mode.
        let toml_str = r##"
[repos]

[tools.claude]
bin = "claude"
args = ["--name", "{name}", "--verbose"]
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let h = config.tool_for("claude").unwrap();
        // User-provided field wins.
        assert_eq!(h.args.len(), 3);
        // Field user did NOT specify falls back to builtin (was None
        // type-default before; now File via merge).
        assert!(
            matches!(h.session_discovery, SessionDiscovery::File { .. }),
            "session_discovery should fall back to builtin Claude's File pattern"
        );
        assert_eq!(
            h.resume_args,
            vec!["--resume".to_string(), "{session_id}".to_string()],
            "resume_args should fall back to builtin"
        );
    }

    #[test]
    fn pi_partial_override_keeps_session_discovery() {
        // Direct repro of the bug Andrew hit: user config sets only
        // bin + prompt_mode for pi, omitting session_discovery. The
        // merge must inherit the builtin pi discovery pattern so muxr
        // save can find sessionIds via ~/.pi/sessions/<pid>.json.
        let toml_str = r##"
[repos]

[tools.pi]
bin = "pi"
prompt_mode = "string"
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let h = config.tool_for("pi").unwrap();
        match h.session_discovery {
            SessionDiscovery::File {
                ref pattern,
                ref id_key,
            } => {
                assert_eq!(pattern, "~/.pi/sessions/{pid}.json");
                assert_eq!(id_key, "sessionId");
            }
            SessionDiscovery::None => {
                panic!("pi partial override must inherit builtin File discovery");
            }
        }
        // Other builtin pi fields preserved
        assert_eq!(
            h.resume_args,
            vec!["--resume".to_string(), "{session_id}".to_string()]
        );
        assert_eq!(h.continue_args, vec!["--continue".to_string()]);
        // User override preserved
        assert_eq!(h.prompt_mode, PromptMode::String);
    }

    #[test]
    fn launch_command_bare() {
        let h = Tool::builtin_claude();
        assert_eq!(h.launch_command(None, None, None), "claude");
    }

    #[test]
    fn launch_command_with_name() {
        let h = Tool::builtin_claude();
        let cmd = h.launch_command(Some("work/api"), None, None);
        assert_eq!(cmd, "claude --name 'work/api'");
    }

    #[test]
    fn launch_command_with_resume_and_model() {
        let h = Tool::builtin_claude();
        let cmd = h.launch_command(
            Some("work/opus"),
            Some("abc-123"),
            Some("claude-opus-4-7"),
        );
        assert_eq!(
            cmd,
            "claude --name 'work/opus' --resume 'abc-123' --model 'claude-opus-4-7'"
        );
    }

    #[test]
    fn launch_command_shell_escapes_quotes() {
        let h = Tool::builtin_claude();
        let cmd = h.launch_command(Some("it's a test"), None, None);
        assert!(cmd.contains("'it'\\''s a test'"));
    }

    #[test]
    fn build_rename_command_interpolates() {
        let h = Tool::builtin_claude();
        let cmd = h.build_rename_command("work/opus").unwrap();
        // Slash commands get raw values -- the harness reads literal
        // keystrokes, not a shell.
        assert_eq!(cmd, "/rename work/opus");
    }

    #[test]
    fn interpolate_raw_no_escaping() {
        assert_eq!(
            interpolate_raw("/model {model}", "model", "claude-opus-4-7"),
            "/model claude-opus-4-7"
        );
    }

    #[test]
    fn interpolate_arg_escapes() {
        assert_eq!(
            interpolate("--model {model}", "model", "claude-opus-4-7"),
            "--model 'claude-opus-4-7'"
        );
    }

    #[test]
    fn build_rename_command_none_when_not_configured() {
        let h = Tool {
            rename_command: None,
            ..Tool::builtin_claude()
        };
        assert!(h.build_rename_command("test").is_none());
    }

    #[test]
    fn resolve_tool_flag_wins() {
        let config = sample_config();
        assert_eq!(config.resolve_tool("work", Some("cursor")), "cursor");
    }

    #[test]
    fn resolve_tool_harness_config() {
        let config = sample_config();
        assert_eq!(config.resolve_tool("personal", None), "opencode");
    }

    #[test]
    fn resolve_tool_default_fallback() {
        let config = sample_config();
        // Unknown harness falls back to default_tool
        assert_eq!(config.resolve_tool("nonexistent", None), "claude");
    }

    #[test]
    fn tool_names_includes_builtin() {
        let config: Config = toml::from_str("[repos]").unwrap();
        let names = config.tool_names();
        assert!(names.contains(&"claude".to_string()));
    }

    #[test]
    fn tool_names_includes_configured() {
        let config = sample_config();
        let names = config.tool_names();
        assert!(names.contains(&"claude".to_string()));
        assert!(names.contains(&"opencode".to_string()));
    }

    // -- Pi runtime tests --

    #[test]
    fn builtin_pi_harness() {
        let h = Tool::builtin_pi();
        assert_eq!(h.bin, "pi");
        assert!(h.args.is_empty());
        assert_eq!(h.continue_args, vec!["--continue".to_string()]);
        assert_eq!(h.prompt_mode, PromptMode::String);
        assert!(h.wrapper.is_none());
        match h.session_discovery {
            SessionDiscovery::File {
                ref pattern,
                ref id_key,
            } => {
                assert_eq!(pattern, "~/.pi/sessions/{pid}.json");
                assert_eq!(id_key, "sessionId");
            }
            _ => panic!("expected file-based session discovery"),
        }
    }

    #[test]
    fn tool_for_returns_builtin_pi() {
        let config: Config = toml::from_str("[repos]").unwrap();
        let h = config.tool_for("pi").unwrap();
        assert_eq!(h.bin, "pi");
        assert_eq!(h.prompt_mode, PromptMode::String);
    }

    #[test]
    fn tool_names_includes_pi_builtin() {
        let config: Config = toml::from_str("[repos]").unwrap();
        let names = config.tool_names();
        assert!(names.contains(&"pi".to_string()));
        assert!(names.contains(&"claude".to_string()));
    }

    #[test]
    fn pi_tool_config_overrides_builtin_with_wrapper() {
        let toml_str = r##"
[repos]

[tools.pi]
bin = "pi"
wrapper = "nono run --profile X --"
prompt_mode = "string"
session_discovery = { type = "none" }
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let h = config.tool_for("pi").unwrap();
        assert_eq!(h.wrapper.as_deref(), Some("nono run --profile X --"));
        assert_eq!(h.prompt_mode, PromptMode::String);
    }

    #[test]
    fn launch_command_pi_with_wrapper_and_string_prompt() {
        // Fixture prompt file with known content.
        let dir = tempfile::tempdir().unwrap();
        let prompt_path = dir.path().join("test-prompt.md");
        let fixture = "fixture content here";
        std::fs::write(&prompt_path, fixture).unwrap();

        // Tool: bin=pi, wrapper set, prompt_mode=string (sandbox profile).
        let tool = Tool {
            bin: "pi".to_string(),
            args: vec![],
            resume_args: vec!["--resume".to_string(), "{session_id}".to_string()],
            model_args: vec!["--model".to_string(), "{model}".to_string()],
            rename_command: Some("/name {name}".to_string()),
            model_switch_command: Some("/model {model}".to_string()),
            compact_command: Some("/compact".to_string()),
            exit_command: Some("/quit".to_string()),
            continue_args: vec!["--continue".to_string()],
            fork_args: vec!["--fork".to_string(), "{session_id}".to_string()],
            session_discovery: SessionDiscovery::None,
            wrapper: Some("nono run --profile X --".to_string()),
            prompt_mode: PromptMode::String,
            supports_add_dirs: Some(false),
        };

        let settings = LaunchSettings {
            append_system_prompt: None,
            append_system_prompt_file: Some(prompt_path.to_string_lossy().to_string()),
            append_system_prompt_files: None,
            add_dirs: vec!["~/docs/should-not-appear".to_string()],
            exclude_dynamic_prompt: false,
            wrapper: None,
        };

        // Resume case: session_id provided -> --resume present, no --continue.
        let cmd = tool
            .launch_command_with_settings(Some("work/pi"), Some("abc-123"), None, &settings)
            .unwrap();
        assert!(
            cmd.starts_with("nono run --profile X -- pi"),
            "wrapper missing or not first; got: {cmd}"
        );
        assert!(
            cmd.contains("--resume 'abc-123'"),
            "expected --resume; got: {cmd}"
        );
        assert!(
            cmd.contains(&format!("--append-system-prompt '{fixture}'")),
            "expected inlined prompt; got: {cmd}"
        );
        assert!(
            !cmd.contains("--append-system-prompt-file"),
            "Pi must not get the file flag; got: {cmd}"
        );
        assert!(
            !cmd.contains("--add-dir"),
            "Pi has no --add-dir; got: {cmd}"
        );
        assert!(
            !cmd.contains("--continue"),
            "should not fall back to --continue when session_id is set; got: {cmd}"
        );

        // Continue/restore case: no session_id -> restore_command falls back to --continue.
        let restore = tool.restore_command(Some("work/pi"), None);
        assert!(
            restore.contains("--continue"),
            "expected --continue fallback; got: {restore}"
        );
        assert!(
            !restore.contains("--resume"),
            "no --resume without session id; got: {restore}"
        );
    }

    #[test]
    fn launch_command_claude_unaffected_by_new_fields() {
        // Default (no wrapper, file prompt mode) keeps current Claude behavior.
        let tool = Tool::builtin_claude();
        let dir = tempfile::tempdir().unwrap();
        let prompt_path = dir.path().join("hp.md");
        std::fs::write(&prompt_path, "x").unwrap();
        let settings = LaunchSettings {
            append_system_prompt: None,
            append_system_prompt_file: Some(prompt_path.to_string_lossy().to_string()),
            append_system_prompt_files: None,
            add_dirs: vec!["/tmp/a".to_string()],
            exclude_dynamic_prompt: false,
            wrapper: None,
        };
        let cmd = tool
            .launch_command_with_settings(Some("v/s"), None, None, &settings)
            .unwrap();
        assert!(cmd.starts_with("claude "), "no wrapper expected: {cmd}");
        assert!(cmd.contains("--append-system-prompt-file"));
        assert!(!cmd.contains("--append-system-prompt '"));
        assert!(cmd.contains("--add-dir '/tmp/a'"));
    }

    #[test]
    fn hooks_parsed() {
        let toml_str = r##"
[repos]

[hooks]
pre_create = ["mise install"]
path = ["~/.local/share/mise/shims"]
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.hooks.pre_create, vec!["mise install"]);
        assert_eq!(config.hooks.path, vec!["~/.local/share/mise/shims"]);
    }

    // -- append_system_prompt_files (array) tests --

    #[test]
    fn launch_settings_deserializes_files_array() {
        // The new field round-trips through TOML correctly.
        let toml_str = r##"
[repos.work]
dir = "~/work"
color = "#fff"

[repos.work.launch]
append_system_prompt_files = ["base.md", "overlay.md"]
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let launch = &config.repos["work"].launch;
        assert_eq!(
            launch.append_system_prompt_files,
            Some(vec!["base.md".to_string(), "overlay.md".to_string()])
        );
        // Singular field stays unset when only array is present.
        assert!(launch.append_system_prompt_file.is_none());
    }

    #[test]
    fn read_and_join_concatenates_with_double_newline() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "# Base").unwrap();
        std::fs::write(&b, "# Overlay").unwrap();
        let result = read_and_join(
            &[
                a.to_string_lossy().to_string(),
                b.to_string_lossy().to_string(),
            ],
            "pi",
        )
        .unwrap();
        assert_eq!(result, "# Base\n\n# Overlay");
    }

    #[test]
    fn pi_array_prompt_inlines_composition() {
        // prompt_mode=String (Pi): two files joined and passed as
        // --append-system-prompt (no temp file, no --append-system-prompt-file).
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.md");
        let overlay = dir.path().join("overlay.md");
        std::fs::write(&base, "base content").unwrap();
        std::fs::write(&overlay, "overlay content").unwrap();

        let tool = Tool::builtin_pi();
        let settings = LaunchSettings {
            append_system_prompt: None,
            append_system_prompt_file: None,
            append_system_prompt_files: Some(vec![
                base.to_string_lossy().to_string(),
                overlay.to_string_lossy().to_string(),
            ]),
            add_dirs: vec![],
            exclude_dynamic_prompt: false,
            wrapper: None,
        };
        let cmd = tool
            .launch_command_with_settings(Some("dunn/test"), None, None, &settings)
            .unwrap();
        assert!(
            cmd.contains("--append-system-prompt 'base content\n\noverlay content'"),
            "expected composed inline prompt; got: {cmd}"
        );
        assert!(
            !cmd.contains("--append-system-prompt-file"),
            "Pi must not get the file flag; got: {cmd}"
        );
    }

    #[test]
    fn claude_array_prompt_writes_temp_file() {
        // prompt_mode=File (Claude): two files → temp file → --append-system-prompt-file.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.md");
        let overlay = dir.path().join("overlay.md");
        std::fs::write(&base, "base").unwrap();
        std::fs::write(&overlay, "overlay").unwrap();

        let tool = Tool::builtin_claude();
        let settings = LaunchSettings {
            append_system_prompt: None,
            append_system_prompt_file: None,
            append_system_prompt_files: Some(vec![
                base.to_string_lossy().to_string(),
                overlay.to_string_lossy().to_string(),
            ]),
            add_dirs: vec![],
            exclude_dynamic_prompt: false,
            wrapper: None,
        };
        let cmd = tool
            .launch_command_with_settings(Some("v/s"), None, None, &settings)
            .unwrap();
        assert!(
            cmd.contains("--append-system-prompt-file"),
            "expected file flag; got: {cmd}"
        );
        assert!(
            !cmd.contains("--append-system-prompt '"),
            "Claude must not get the inline flag; got: {cmd}"
        );
        // Verify the temp file contains the joined content.
        let tmp = std::env::temp_dir().join("muxr-composed-system-prompt.md");
        let written = std::fs::read_to_string(&tmp).unwrap();
        assert_eq!(written, "base\n\noverlay");
    }

    #[test]
    fn array_takes_precedence_over_singular() {
        // When both fields are set, the array wins and the singular is ignored.
        let dir = tempfile::tempdir().unwrap();
        let arr_file = dir.path().join("arr.md");
        let sing_file = dir.path().join("sing.md");
        std::fs::write(&arr_file, "from array").unwrap();
        std::fs::write(&sing_file, "from singular").unwrap();

        let tool = Tool::builtin_pi();
        let settings = LaunchSettings {
            append_system_prompt: None,
            append_system_prompt_file: Some(sing_file.to_string_lossy().to_string()),
            append_system_prompt_files: Some(vec![arr_file.to_string_lossy().to_string()]),
            add_dirs: vec![],
            exclude_dynamic_prompt: false,
            wrapper: None,
        };
        let cmd = tool
            .launch_command_with_settings(Some("v/s"), None, None, &settings)
            .unwrap();
        assert!(
            cmd.contains("'from array'"),
            "array content must win; got: {cmd}"
        );
        assert!(
            !cmd.contains("from singular"),
            "singular must be suppressed; got: {cmd}"
        );
    }
}
