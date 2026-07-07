use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_tool")]
    pub default_tool: String,
    // Defaults to empty so a fresh `muxr init` config (no repos yet) parses;
    // using an unknown repo then fails with a clear "unknown repo" error
    // rather than a serde "missing field repos" at load time.
    #[serde(default)]
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
    /// Optional companion pane created beside the runtime at launch and
    /// recreated on restore. Global default; a per-repo
    /// `[repos.<name>.companion]` overrides it. Absent -> no companion.
    /// See ADR 0004.
    #[serde(default)]
    pub companion: Option<Companion>,
    /// Namespace roots scanned for drop-in per-repo `muxr.toml` fragments, so a
    /// repo carries its own muxr entry with no central edit. Empty `roots` (the
    /// default, and any config with no `[discovery]`) means no discovery: the
    /// single-file, pre-3.6 behavior. A root absent on this machine is simply
    /// not discovered -- zero cross-machine knowledge. See `discover_and_merge`.
    #[serde(default)]
    pub discovery: Discovery,
    /// Readiness-gate thresholds for `muxr upgrade`/`recycle`. Absent
    /// `[readiness]` -> the built-in defaults, byte-identical to pre-3.6
    /// behavior. Currently exposes `stale_busy_secs` so an operator can reclaim
    /// interrupted-but-quiet sessions sooner without a rebuild.
    #[serde(default)]
    pub readiness: ReadinessConfig,
}

/// Thresholds for the upgrade/recycle readiness gate. Defaults reproduce the
/// built-in behavior exactly (`stale_busy_secs` = [`state::STALE_BUSY_SECS`]);
/// an operator lowers `stale_busy_secs` to reclaim a session whose agent turn
/// was interrupted (a `busy` state file that never got its `idle`) sooner. This
/// stays conservative: a stale-busy file resolves to `Unknown` and falls
/// through to the tmux-activity floor, which only returns `Safe` once the pane
/// has actually been quiet for `min_idle`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessConfig {
    /// A `busy` state file older than this (seconds) is treated as stale (a
    /// likely crashed/interrupted session that fired `busy` but never wrote
    /// `idle`) and falls through to the activity floor instead of blocking
    /// upgrade. Default 3600 (1 hour), the pre-3.6 hardcoded value.
    #[serde(default = "default_stale_busy_secs")]
    pub stale_busy_secs: u64,
}

fn default_stale_busy_secs() -> u64 {
    crate::state::STALE_BUSY_SECS
}

impl Default for ReadinessConfig {
    // Manual (not derived) so `Default::default()` yields 3600, NOT 0 -- a 0
    // threshold would make every `busy` file instantly stale. Reuses the same
    // default fn as serde so there is a single source for the default.
    fn default() -> Self {
        Self {
            stale_busy_secs: default_stale_busy_secs(),
        }
    }
}

/// Per-repo config discovery. `roots` are namespace directories walked (bounded
/// to 2 levels: `<root>/<namespace>/<repo>`) for a `fragment` file at a git
/// repo root; each qualifying fragment's `repos`/`remotes` are merged into the
/// central config. Absent `[discovery]` -> empty `roots` -> discovery disabled.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    /// Namespace directories to scan. Each entry is tilde-expanded. A root that
    /// does not exist on this machine is skipped, not an error.
    #[serde(default)]
    pub roots: Vec<String>,
    /// Fragment file name looked for at each candidate repo root. Default
    /// `muxr.toml`. (Note: `Default::default` leaves this empty -- serde's
    /// default fn only fires on deserialize -- so the walk falls back to
    /// `muxr.toml` when it is empty.)
    #[serde(default = "default_fragment_name")]
    pub fragment: String,
}

fn default_fragment_name() -> String {
    "muxr.toml".to_string()
}

/// External chooser delegation. The built-in TUI does far more than a generic
/// tmux picker (opens dormant campaigns, recycle/archive/rename); `command` is
/// a thin opt-out for users who prefer their own picker for plain attach, NOT
/// a full replacement.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct Repo {
    pub dir: String,
    pub color: String,
    /// Override default_tool for this repo.
    #[serde(default)]
    pub tool: Option<String>,
    /// Tool-launch settings. Passed through to the runtime at session start.
    #[serde(default)]
    pub launch: LaunchSettings,
    /// Per-repo companion-pane override of the global `[companion]`.
    #[serde(default)]
    pub companion: Option<Companion>,
    /// Open extension namespace: arbitrary TOML muxr carries but never
    /// interprets, handed to extensions verbatim (the resolver intent's `ext`
    /// field and the `muxr config` query). This is how a repo declares
    /// extension/preference data -- chrome (statusline glyph/color), launcher
    /// hints -- as CONFIG, with no muxr schema change. The core keys above keep
    /// `deny_unknown_fields` (a typo in `dir`/`color`/... still fails loud);
    /// only this one namespace is deliberately open.
    #[serde(default)]
    pub ext: toml::Table,
}

/// Settings passed to the tool on launch. Muxr passes these through
/// to the runtime -- it does not interpret them.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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

/// An optional companion pane: a review/preview pane created beside the runtime
/// at launch and recreated on restore (config-driven, opt-in, per-repo
/// overridable). muxr only splits the pane and runs `cmd`; what it renders is
/// the operator's concern. See ADR 0004.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Companion {
    /// Off by default; must be true for the pane to be created.
    #[serde(default)]
    pub enabled: bool,
    /// Command run in the companion pane. Templated with the same tokens as
    /// `[session_env]`: `{session}` `{repo}` `{campaign}` `{session_slug}` `{dir}`.
    pub cmd: String,
    /// Split direction: "h" (side-by-side) or "v" (stacked). Default "h".
    #[serde(default = "default_companion_side")]
    pub side: String,
    /// Companion pane size, as a percentage of the split. Default 40.
    #[serde(default = "default_companion_size")]
    pub size: u8,
}

fn default_companion_side() -> String {
    "h".to_string()
}

fn default_companion_size() -> u8 {
    40
}

/// A `[companion]` resolved for a concrete session: the literal command (tokens
/// interpolated) plus its geometry. Returned by `Config::companion_for`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCompanion {
    pub cmd: String,
    pub side: String,
    pub size: u8,
}

/// File-based session-discovery payload. A standalone `deny_unknown_fields`
/// struct (not inline enum-variant fields) so a typo'd sibling key inside
/// `[tools.*.session_discovery]` is REJECTED rather than silently dropped --
/// the internally-tagged `SessionDiscovery` enum can't carry
/// `deny_unknown_fields` itself (serde forbids it with `tag`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileDiscovery {
    /// Path pattern with `{pid}` placeholder.
    pub pattern: String,
    /// JSON key containing the session ID.
    pub id_key: String,
}

/// How to discover harness session IDs from running processes.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SessionDiscovery {
    /// Walk the process tree, look for a session file per PID.
    File(FileDiscovery),
    /// No session discovery (tool doesn't support resume).
    None,
}

/// Configuration for a harness (AI coding tool).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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
    /// How to probe session readiness before upgrade.
    #[serde(default = "default_readiness_none")]
    pub readiness: ReadinessProbe,
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

/// File readiness-probe payload. Standalone `deny_unknown_fields` struct so a
/// typo inside `[tools.*.readiness]` (e.g. `idle_valeu` for `idle_value`, which
/// would silently disable the quiet-period guard and let muxr upgrade a busy
/// session) is rejected, not dropped. See `FileDiscovery` for why the payload
/// can't live as inline enum-variant fields.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileProbe {
    /// Path pattern with `{session_id}` (preferred) or `{pid}` placeholder.
    pub pattern: String,
    /// JSON key containing the state string.
    pub state_key: String,
    /// Value meaning "safe to upgrade", e.g. `"idle"`.
    pub idle_value: String,
    /// Optional epoch-seconds key for quiet-period enforcement.
    #[serde(default)]
    pub since_key: Option<String>,
}

/// Command readiness-probe payload. Standalone `deny_unknown_fields` struct
/// (same rationale as `FileProbe`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CommandProbe {
    /// Command + args; `{session_id}` and `{pid}` are interpolated.
    pub argv: Vec<String>,
}

/// How to probe a session for upgrade readiness.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ReadinessProbe {
    /// Read a normalized state file the runtime's hooks maintain.
    File(FileProbe),
    /// Escape hatch: a runtime that exposes readiness via a CLI. Exit 0 = safe.
    Command(CommandProbe),
    /// No runtime probe — core uses the universal tmux-activity floor only.
    /// This is the type default; in a user `[tools.<name>]` block it means
    /// "inherit the builtin probe" (see `merge_tool_with_builtin`).
    None,
    /// Explicit opt-out: floor only, and do NOT inherit the builtin probe.
    /// Resolved to `None` during merge, so the classifier never sees it.
    Disabled,
}

fn default_readiness_none() -> ReadinessProbe {
    ReadinessProbe::None
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
        readiness: match user.readiness {
            // Unset (the type default) inherits the builtin probe...
            ReadinessProbe::None => builtin.readiness,
            // ...but an explicit `type = "disabled"` opts out without inheriting.
            ReadinessProbe::Disabled => ReadinessProbe::None,
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

/// Top-level config keys muxr has renamed across versions, as `(old, new)`.
/// Now that the config structs `deny_unknown_fields`, an old key is a hard
/// parse error -- but serde's generic "unknown field" message doesn't say what
/// to do. On a parse failure we scan the raw TOML for these so the error can
/// name the replacement. Append future renames here as the schema evolves.
const KNOWN_RENAMES: &[(&str, &str)] = &[
    // muxr 3.x renamed the repo table `[harnesses.*]` -> `[repos.*]`.
    ("harnesses", "repos"),
];

/// If `content` is parseable TOML that still uses a renamed top-level key,
/// return an actionable hint naming the replacement(s). Returns `None` when
/// the content has no known-old keys (or isn't even valid TOML -- then the
/// raw parse error stands on its own). This runs only on the error path, so
/// it never costs the happy path a second parse.
fn rename_hint(content: &str) -> Option<String> {
    let table: toml::Table = toml::from_str(content).ok()?;
    let hits: Vec<String> = KNOWN_RENAMES
        .iter()
        .filter(|(old, _)| table.contains_key(*old))
        .map(|(old, new)| format!("  `[{old}.*]` was renamed to `[{new}.*]`"))
        .collect();
    if hits.is_empty() {
        return None;
    }
    Some(format!(
        "hint: this config uses key(s) muxr has renamed:\n{}\nRename them and retry \
         (the old names are no longer accepted).",
        hits.join("\n")
    ))
}

/// One shipped adapter file: `extensions/adapters/<name>.toml` is a single
/// `[tools.<name>]` block, so it deserializes into this one-entry table.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterFile {
    #[serde(default)]
    tools: HashMap<String, Tool>,
}

/// The runtime adapters muxr ships in the box.
///
/// These ARE the `extensions/adapters/*.toml` files (the same ones documented
/// in `extensions/README.md`), embedded at compile time and parsed once. The
/// shipped TOML is the single source of truth -- core no longer carries a
/// hand-written per-runtime struct, and `tool_for`/`tool_names` resolve through
/// this table generically rather than matching on a hardcoded tool name.
///
/// Only claude + pi ship as defaults (matching pre-3.1 behavior byte-for-byte);
/// opencode.toml in that dir is a worked example, not a default. A malformed
/// shipped file is a build invariant violation (covered by `shipped_adapters_*`
/// tests), so parse failure panics rather than degrading silently.
fn builtin_adapters() -> &'static HashMap<String, Tool> {
    static ADAPTERS: OnceLock<HashMap<String, Tool>> = OnceLock::new();
    ADAPTERS.get_or_init(|| {
        let mut m = HashMap::new();
        for src in [
            include_str!("../extensions/adapters/claude.toml"),
            include_str!("../extensions/adapters/pi.toml"),
        ] {
            let parsed: AdapterFile = toml::from_str(src).expect("shipped adapter TOML must parse");
            m.extend(parsed.tools);
        }
        m
    })
}

impl Tool {
    /// The shipped Claude Code adapter. Thin accessor over `builtin_adapters()`
    /// (the TOML is authoritative); used only by tests that assert its fields.
    #[cfg(test)]
    pub fn builtin_claude() -> Self {
        builtin_adapters()
            .get("claude")
            .cloned()
            .expect("claude adapter ships in the box")
    }

    /// The shipped Pi adapter. Thin accessor over `builtin_adapters()`.
    #[cfg(test)]
    pub fn builtin_pi() -> Self {
        builtin_adapters()
            .get("pi")
            .cloned()
            .expect("pi adapter ships in the box")
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

    /// Build the launch command with harness-specific settings from the harness.
    ///
    /// If the tool has a `wrapper` set, the final command is
    /// `<wrapper> <launch_command> <settings flags...>`.
    /// `prompt_mode` controls how `append_system_prompt_file` is delivered:
    /// File (default) passes `--append-system-prompt-file <path>` (Claude),
    /// String reads the file contents and inlines them into
    /// `--append-system-prompt <content>` (Pi, which lacks a file variant).
    /// A runtime that has no `--add-dir` flag opts out via `supports_add_dirs =
    /// false` (e.g. Pi); `add_dirs` are then skipped. This is capability-driven
    /// (`emits_add_dirs()`), NOT a per-bin branch.
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
#[serde(deny_unknown_fields)]
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
        let mut config = Self::parse(&content, &path.display().to_string())?;
        config.discover_and_merge()?;
        Ok(config)
    }

    /// Parse + validate config TOML. Split from `load()` so the strict-parse,
    /// rename diagnostics, and collision checks are exercised by tests without
    /// a config file on disk. `source` is the path/label used in error messages.
    ///
    /// The structs `deny_unknown_fields`, so an unknown or renamed key is a hard
    /// error rather than a silently-dropped table (the failure mode of #3, where
    /// `[harnesses.*]` parsed to an empty `repos` and surfaced later as a baffling
    /// "unknown repo"). On parse failure we enrich the serde error with a rename
    /// hint when the raw TOML still uses a known-old key.
    pub fn parse(content: &str, source: &str) -> Result<Self> {
        let config: Config = toml::from_str(content).map_err(|e| {
            let base = format!("Failed to parse {source}:\n{e}");
            match rename_hint(content) {
                Some(hint) => anyhow::anyhow!("{base}\n\n{hint}"),
                None => anyhow::anyhow!("{base}"),
            }
        })?;

        config.check_collisions()?;
        Ok(config)
    }

    /// Validate no name collisions between repos, remotes, and tools (and that
    /// no repo/tool name shadows a built-in command). Split out of `parse` so
    /// the same rules run after discovery merges fragments into the config.
    fn check_collisions(&self) -> Result<()> {
        for name in self.remotes.keys() {
            if self.repos.contains_key(name) {
                anyhow::bail!("Name collision: '{name}' is defined as both a repo and a remote");
            }
        }
        for name in self.tools.keys() {
            if self.repos.contains_key(name) {
                anyhow::bail!("Name collision: '{name}' is defined as both a tool and a repo");
            }
            if self.remotes.contains_key(name) {
                anyhow::bail!("Name collision: '{name}' is defined as both a remote and a repo");
            }
            if RESERVED_NAMES.contains(&name.as_str()) {
                anyhow::bail!("Repo name '{name}' is reserved (conflicts with built-in command)");
            }
        }
        Ok(())
    }

    /// Parse one per-repo `muxr.toml` fragment. Same strict-parse + rename-hint
    /// enrichment as `parse`, but NO collision check: a fragment's names are
    /// validated only after the full merge (`check_collisions` at the end of
    /// `discover_and_merge`), since collisions are a whole-config property.
    fn parse_fragment(content: &str, source: &str) -> Result<Self> {
        toml::from_str(content).map_err(|e| {
            let base = format!("Failed to parse fragment {source}:\n{e}");
            match rename_hint(content) {
                Some(hint) => anyhow::anyhow!("{base}\n\n{hint}"),
                None => anyhow::anyhow!("{base}"),
            }
        })
    }

    /// Discover drop-in per-repo `muxr.toml` fragments under the configured
    /// `[discovery]` roots and merge their `repos`/`remotes` into this config.
    ///
    /// No roots -> immediate no-op (single-file, pre-3.6 behavior). Otherwise
    /// each root is tilde-expanded and walked exactly 2 levels deep
    /// (`<root>/<namespace>/<repo>`); a candidate `<repo>/<fragment>` qualifies
    /// only when both that file and a `<repo>/.git` entry exist (a git repo
    /// root -- `.git` may be a dir or a worktree file, so `.exists()`). A root
    /// or namespace dir that cannot be read is skipped, not an error, so a repo
    /// absent on this machine is simply not discovered. Fragments are merged in
    /// sorted path order for determinism. Only `repos` and `remotes` are taken
    /// from a fragment; any other field it carries is ignored (a fragment must
    /// not redefine hooks/extensions/etc). A fragment name already present in
    /// the config is a hard error naming the key and the fragment path.
    fn discover_and_merge(&mut self) -> Result<()> {
        if self.discovery.roots.is_empty() {
            return Ok(());
        }

        // `Default::default` leaves `fragment` empty (serde's default fn only
        // fires on deserialize), so fall back to the shipped name here.
        let fragment_name = if self.discovery.fragment.is_empty() {
            "muxr.toml"
        } else {
            self.discovery.fragment.as_str()
        };

        // Bounded 2-level walk (no walkdir/glob dep): root -> namespace -> repo.
        let mut fragments: Vec<PathBuf> = Vec::new();
        for root in &self.discovery.roots {
            let root_path = PathBuf::from(shellexpand::tilde(root).to_string());
            let Ok(namespaces) = std::fs::read_dir(&root_path) else {
                continue; // missing/unreadable root: skipped, not an error
            };
            for namespace in namespaces.flatten() {
                let ns_path = namespace.path();
                let Ok(repos) = std::fs::read_dir(&ns_path) else {
                    continue; // unreadable namespace dir: skip
                };
                for repo in repos.flatten() {
                    let repo_path = repo.path();
                    let fragment = repo_path.join(fragment_name);
                    // Qualifies only at a git repo root carrying the fragment.
                    if fragment.exists() && repo_path.join(".git").exists() {
                        fragments.push(fragment);
                    }
                }
            }
        }

        // Deterministic merge order regardless of read_dir ordering.
        fragments.sort();

        for fragment in &fragments {
            let source = fragment.display().to_string();
            let content = std::fs::read_to_string(fragment)
                .with_context(|| format!("Failed to read fragment {source}"))?;
            let parsed = Self::parse_fragment(&content, &source)?;
            // Only repos and remotes cross the fragment boundary.
            for (name, repo) in parsed.repos {
                if self.repos.contains_key(&name) {
                    anyhow::bail!(
                        "Duplicate repo '{name}' in fragment {source} (already defined)"
                    );
                }
                self.repos.insert(name, repo);
            }
            for (name, remote) in parsed.remotes {
                if self.remotes.contains_key(&name) {
                    anyhow::bail!(
                        "Duplicate remote '{name}' in fragment {source} (already defined)"
                    );
                }
                self.remotes.insert(name, remote);
            }
        }

        self.check_collisions()
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
        let (repo, campaign) = session_name.split_once('/').unwrap_or((session_name, ""));
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

    /// Resolve the companion pane for a concrete session, or `None` if none
    /// applies. A repo-level `[repos.<repo>.companion]` wins over the global
    /// `[companion]`; returns `None` when neither is set or the resolved one is
    /// disabled. Tokens `{session}` `{repo}` `{campaign}` `{session_slug}`
    /// `{dir}` are interpolated into `cmd` (same slug rule as `session_env_for`).
    pub fn companion_for(&self, session_name: &str, dir: &str) -> Option<ResolvedCompanion> {
        let (repo, campaign) = session_name.split_once('/').unwrap_or((session_name, ""));
        let companion = self
            .repos
            .get(repo)
            .and_then(|r| r.companion.as_ref())
            .or(self.companion.as_ref())?;
        if !companion.enabled {
            return None;
        }
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
        let cmd = companion
            .cmd
            .replace("{session_slug}", &slug)
            .replace("{session}", session_name)
            .replace("{repo}", repo)
            .replace("{campaign}", campaign)
            .replace("{dir}", dir);
        Some(ResolvedCompanion {
            cmd,
            side: companion.side.clone(),
            size: companion.size,
        })
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
        let builtin = builtin_adapters().get(tool).cloned();
        match (self.tools.get(tool).cloned(), builtin) {
            (Some(user), Some(builtin)) => Some(merge_tool_with_builtin(user, builtin)),
            (Some(user), None) => Some(user),
            (None, Some(builtin)) => Some(builtin),
            (None, None) => None,
        }
    }

    /// All configured harness names (explicit user tools + shipped adapters).
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        for name in builtin_adapters().keys() {
            if !names.contains(name) {
                names.push(name.clone());
            }
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
#
# This file is validated STRICTLY against the running muxr: an unknown,
# misspelled, or renamed key is a hard error that names the offending key
# (no silent drops). Keep muxr and this config in step.

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

# Optional companion pane beside the runtime, recreated faithfully on restore.
# Global default here, or per-repo via [repos.<name>.companion]. `cmd` is
# templated with {session} {repo} {campaign} {session_slug} {dir}. See ADR 0004.
#
# [companion]
# enabled = true
# cmd = "my-previewer {dir}"
# side = "h"   # "h" side-by-side, "v" stacked
# size = 40    # companion pane size, percent

# Upgrade/recycle readiness gate. Absent -> built-in defaults (unchanged).
# Lower stale_busy_secs to reclaim a session whose agent turn was interrupted
# (a `busy` state file that never got its `idle`) sooner; still corroborated
# against pane quiet (min_idle) via the activity floor.
#
# [readiness]
# stale_busy_secs = 600   # default 3600 (1h)
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

    #[test]
    fn companion_global_resolves_and_interpolates() {
        let config: Config = toml::from_str(
            "[repos]\n\n[companion]\nenabled = true\ncmd = \"prev {repo} {campaign} {session_slug} {dir}\"\n",
        )
        .unwrap();
        let c = config
            .companion_for("work/in-place/fix", "/tmp/d")
            .expect("global companion resolves");
        assert_eq!(c.cmd, "prev work in-place/fix work-in-place-fix /tmp/d");
        assert_eq!(c.side, "h"); // default
        assert_eq!(c.size, 40); // default
    }

    #[test]
    fn companion_repo_override_wins_and_global_is_fallback() {
        let config: Config = toml::from_str(
            "[repos.work]\ndir = \"~/w\"\ncolor = \"#fff\"\n\n\
             [repos.work.companion]\nenabled = true\ncmd = \"repo-prev\"\nside = \"v\"\nsize = 30\n\n\
             [companion]\nenabled = true\ncmd = \"global-prev\"\n",
        )
        .unwrap();
        let c = config.companion_for("work/x", "/d").expect("repo companion");
        assert_eq!((c.cmd.as_str(), c.side.as_str(), c.size), ("repo-prev", "v", 30));
        // a repo with no override falls back to the global companion
        let g = config
            .companion_for("other/y", "/d")
            .expect("global fallback");
        assert_eq!(g.cmd, "global-prev");
    }

    #[test]
    fn companion_disabled_is_none() {
        let config: Config =
            toml::from_str("[repos]\n\n[companion]\nenabled = false\ncmd = \"x\"\n").unwrap();
        assert!(config.companion_for("work/x", "/d").is_none());
    }

    #[test]
    fn companion_absent_is_none() {
        let config: Config = toml::from_str("[repos]").unwrap();
        assert!(config.companion_for("work/x", "/d").is_none());
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
    fn repo_ext_namespace_is_open_but_core_stays_strict() {
        // The `ext` namespace accepts arbitrary nested tables -- adding a
        // preference is a config change, not a muxr rebuild.
        let c = Config::parse(
            "[repos.work]\ndir = \"~/w\"\ncolor = \"#111\"\n\
             [repos.work.ext.chrome]\nglyph_codepoint = \"100002\"\nfamily = \"Work Mark\"\n",
            "test",
        )
        .expect("open ext namespace parses");
        let chrome = c.repos["work"].ext["chrome"]
            .as_table()
            .expect("chrome is a table");
        assert_eq!(chrome["glyph_codepoint"].as_str(), Some("100002"));

        // A repo with no ext gets an empty table, not an error.
        let c2 = Config::parse("[repos.w]\ndir = \"~/w\"\ncolor = \"#111\"\n", "test").unwrap();
        assert!(c2.repos["w"].ext.is_empty());

        // But a typo in a CORE key still fails loud (deny_unknown_fields intact).
        let err = Config::parse(
            "[repos.work]\ndir = \"~/w\"\ncolor = \"#111\"\ncolr = \"oops\"\n",
            "test",
        )
        .unwrap_err();
        let msg = format!("{err}").to_lowercase();
        assert!(msg.contains("colr") || msg.contains("unknown"), "got: {msg}");
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

    #[test]
    fn default_template_parses_as_valid_config() {
        // `muxr init` writes this template; it must deserialize cleanly (the
        // commented [extensions]/[session_env]/[chooser] examples must not be
        // active, and the active body must be valid).
        let template = Config::default_template();
        let cfg: Config = toml::from_str(&template).expect("default template must parse");
        assert_eq!(cfg.default_tool, "claude");
        // No extensions/env/chooser configured by default -> bare launcher.
        assert!(cfg.extensions.resolver.is_none());
        assert!(cfg.extensions.make_durable.is_none());
        assert!(cfg.session_env.is_empty());
        assert!(cfg.chooser.command.is_none());
        assert!(cfg.companion.is_none());
    }

    // -- Harness config tests --

    #[test]
    fn builtin_claude_harness() {
        let h = Tool::builtin_claude();
        assert_eq!(h.bin, "claude");
        assert_eq!(h.rename_command, Some("/rename {name}".to_string()));
        assert!(matches!(h.session_discovery, SessionDiscovery::File(_)));
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
            matches!(h.session_discovery, SessionDiscovery::File(_)),
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
            SessionDiscovery::File(ref d) => {
                assert_eq!(d.pattern, "~/.pi/sessions/{pid}.json");
                assert_eq!(d.id_key, "sessionId");
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
        let cmd = h.launch_command(Some("work/opus"), Some("abc-123"), Some("claude-opus-4-7"));
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
    fn shipped_adapters_are_exactly_claude_and_pi() {
        // The shipped (compile-time-embedded) adapter set is what `muxr` offers
        // out of the box. Lock it to claude + pi: pre-3.1 hardcoded exactly
        // these, and opencode.toml ships as a worked EXAMPLE, not a default.
        let mut keys: Vec<&str> = builtin_adapters().keys().map(|s| s.as_str()).collect();
        keys.sort();
        assert_eq!(keys, vec!["claude", "pi"]);
    }

    #[test]
    fn user_only_tool_resolves_without_a_builtin() {
        // A runtime with no shipped adapter (e.g. opencode) still resolves from
        // the user's [tools.*] block alone -- the (Some user, None builtin) arm.
        let config: Config = toml::from_str("[tools.opencode]\nbin = \"opencode\"").unwrap();
        let t = config.tool_for("opencode").expect("user tool resolves");
        assert_eq!(t.bin, "opencode");
    }

    #[test]
    fn builtin_pi_harness() {
        let h = Tool::builtin_pi();
        assert_eq!(h.bin, "pi");
        assert!(h.args.is_empty());
        assert_eq!(h.continue_args, vec!["--continue".to_string()]);
        assert_eq!(h.prompt_mode, PromptMode::String);
        assert!(h.wrapper.is_none());
        match h.session_discovery {
            SessionDiscovery::File(ref d) => {
                assert_eq!(d.pattern, "~/.pi/sessions/{pid}.json");
                assert_eq!(d.id_key, "sessionId");
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
            exit_command: Some("/quit".to_string()),
            continue_args: vec!["--continue".to_string()],
            fork_args: vec!["--fork".to_string(), "{session_id}".to_string()],
            session_discovery: SessionDiscovery::None,
            wrapper: Some("nono run --profile X --".to_string()),
            prompt_mode: PromptMode::String,
            supports_add_dirs: Some(false),
            readiness: ReadinessProbe::None,
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
    fn readiness_probe_inherits_from_builtin_when_user_omits_it() {
        // A partial [tools.claude] block without [readiness] must inherit the
        // builtin Claude probe (File), not collapse to None.
        let toml_str = r##"
[repos]

[tools.claude]
bin = "claude"
args = ["--name", "{name}", "--verbose"]
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let h = config.tool_for("claude").unwrap();
        assert!(
            matches!(h.readiness, ReadinessProbe::File(_)),
            "readiness should fall back to builtin File probe; got {:?}",
            h.readiness
        );
    }

    #[test]
    fn readiness_probe_user_override_wins() {
        // A [tools.claude] block with an explicit [readiness] overrides the builtin.
        let toml_str = r##"
[repos]

[tools.claude]
bin = "claude"

[tools.claude.readiness]
type = "command"
argv = ["my-probe", "--session", "{session_id}"]
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let h = config.tool_for("claude").unwrap();
        assert!(
            matches!(h.readiness, ReadinessProbe::Command(_)),
            "user readiness override should win; got {:?}",
            h.readiness
        );
    }

    #[test]
    fn readiness_probe_disabled_opts_out_without_inheriting() {
        // `type = "disabled"` is an explicit opt-out: it resolves to None
        // (floor only) and must NOT inherit the builtin File probe.
        let toml_str = r##"
[repos]

[tools.claude]
bin = "claude"

[tools.claude.readiness]
type = "disabled"
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let h = config.tool_for("claude").unwrap();
        assert!(
            matches!(h.readiness, ReadinessProbe::None),
            "disabled must resolve to None (no builtin inherit); got {:?}",
            h.readiness
        );
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

    // -- Strict parse + rename advice (#3) --

    #[test]
    fn unknown_top_level_key_is_rejected() {
        // The #3 failure mode: a renamed/typo'd top-level table used to be
        // silently dropped (repos defaulted empty -> baffling "unknown repo"
        // later). deny_unknown_fields now makes it a hard parse error.
        let err = toml::from_str::<Config>("[harnesses.work]\ndir = \"~/w\"\ncolor = \"#fff\"\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("harnesses") || err.contains("unknown field"),
            "expected an unknown-field error naming the key; got: {err}"
        );
    }

    #[test]
    fn unknown_nested_key_is_rejected() {
        // A typo in an optional field inside [repos.<n>] (here `colour` for
        // `color`) used to be silently ignored; now it's rejected.
        let err = toml::from_str::<Config>(
            "[repos.work]\ndir = \"~/w\"\ncolor = \"#fff\"\ncolour = \"#000\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("colour") || err.contains("unknown field"),
            "expected an unknown-field error for the nested typo; got: {err}"
        );
    }

    #[test]
    fn parse_enriches_renamed_key_with_hint() {
        // Config::parse routes a renamed key through rename_hint, so the error
        // names the replacement instead of only "unknown field harnesses".
        let err = Config::parse(
            "[harnesses.work]\ndir = \"~/w\"\ncolor = \"#fff\"\n",
            "<test>",
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("`[harnesses.*]` was renamed to `[repos.*]`"),
            "expected the rename hint; got: {err}"
        );
    }

    #[test]
    fn rename_hint_none_for_clean_config() {
        // A config with no known-old keys yields no hint (the raw parse error,
        // if any, stands alone).
        assert!(rename_hint("[repos.work]\ndir = \"~/w\"\ncolor = \"#fff\"\n").is_none());
        // Also None when the content isn't valid TOML at all.
        assert!(rename_hint("this is not = = toml").is_none());
    }

    #[test]
    fn parse_accepts_a_known_good_config() {
        // The full happy path through Config::parse: real-shaped config with
        // repos, a remote, hooks, a launch block, and a partial tool override.
        let toml_str = r##"
default_tool = "claude"

[hooks]
pre_create = ["mise install"]
path = ["~/.local/share/mise/shims"]

[repos.work]
dir = "~/w"
color = "#fff"

[repos.work.launch]
add_dirs = ["~/docs"]
append_system_prompt_files = ["base.md", "HARNESS.md"]
exclude_dynamic_prompt = true

[remotes.lab]
project = "p"
zone = "z"
user = "u"
color = "#000"

[tools.pi]
bin = "pi"
prompt_mode = "string"
"##;
        let cfg = Config::parse(toml_str, "<test>").expect("known-good config must parse");
        assert_eq!(cfg.repos.len(), 1);
        assert_eq!(cfg.remotes.len(), 1);
        assert!(cfg.repos["work"].launch.exclude_dynamic_prompt);
    }

    #[test]
    fn parse_still_catches_name_collisions() {
        // The collision validation moved into parse(); confirm it still fires.
        let err = Config::parse(
            "[repos.lab]\ndir = \"~/l\"\ncolor = \"#fff\"\n\n[remotes.lab]\nproject = \"p\"\nzone = \"z\"\nuser = \"u\"\ncolor = \"#000\"\n",
            "<test>",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Name collision"), "got: {err}");
    }

    #[test]
    fn discovery_empty_roots_is_noop() {
        // No [discovery] -> empty roots -> discover_and_merge changes nothing.
        let mut config = Config::parse(
            "[repos.base]\ndir = \"~/b\"\ncolor = \"#fff\"\n",
            "<test>",
        )
        .unwrap();
        assert!(config.discovery.roots.is_empty());
        config.discover_and_merge().unwrap();
        assert_eq!(config.repos.len(), 1);
        assert!(config.repos.contains_key("base"));
    }

    #[test]
    fn discovery_merges_fragment_repos() {
        // <root>/ns/repoA/{.git/, muxr.toml} with [repos.alpha] + ext.
        let root = tempfile::tempdir().unwrap();
        let repo_a = root.path().join("ns").join("repoA");
        std::fs::create_dir_all(repo_a.join(".git")).unwrap();
        std::fs::write(
            repo_a.join("muxr.toml"),
            "[repos.alpha]\ndir = \"~/a\"\ncolor = \"#123456\"\n\n[repos.alpha.ext.chrome]\nglyph = \"A\"\n",
        )
        .unwrap();

        let mut config = Config::parse(
            "[repos.base]\ndir = \"~/b\"\ncolor = \"#fff\"\n",
            "<test>",
        )
        .unwrap();
        config.discovery.roots = vec![root.path().to_string_lossy().into_owned()];
        config.discover_and_merge().unwrap();

        assert!(config.repos.contains_key("alpha"), "fragment repo merged");
        assert!(config.repos.contains_key("base"), "base repo retained");
        let alpha = &config.repos["alpha"];
        assert_eq!(alpha.color, "#123456");
        // The open ext namespace survives the merge verbatim.
        assert!(
            alpha.ext.contains_key("chrome"),
            "repo ext survived the merge"
        );
    }

    #[test]
    fn discovery_ignores_non_git_dir() {
        // A muxr.toml in a dir WITHOUT .git is not a git repo root -> skipped.
        let root = tempfile::tempdir().unwrap();
        let not_a_repo = root.path().join("ns").join("plain");
        std::fs::create_dir_all(&not_a_repo).unwrap();
        std::fs::write(
            not_a_repo.join("muxr.toml"),
            "[repos.ghost]\ndir = \"~/g\"\ncolor = \"#000\"\n",
        )
        .unwrap();

        let mut config = Config::parse("[repos]\n", "<test>").unwrap();
        config.discovery.roots = vec![root.path().to_string_lossy().into_owned()];
        config.discover_and_merge().unwrap();

        assert!(
            !config.repos.contains_key("ghost"),
            "fragment without .git must not be merged"
        );
        assert!(config.repos.is_empty());
    }

    #[test]
    fn discovery_duplicate_repo_errors() {
        // A fragment redefining a name already in the base config is an error.
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("ns").join("dup");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(
            repo.join("muxr.toml"),
            "[repos.base]\ndir = \"~/other\"\ncolor = \"#abc\"\n",
        )
        .unwrap();

        let mut config = Config::parse(
            "[repos.base]\ndir = \"~/b\"\ncolor = \"#fff\"\n",
            "<test>",
        )
        .unwrap();
        config.discovery.roots = vec![root.path().to_string_lossy().into_owned()];
        let err = config.discover_and_merge().unwrap_err().to_string();
        assert!(err.contains("Duplicate repo 'base'"), "got: {err}");
    }

    #[test]
    fn readiness_file_probe_rejects_unknown_key() {
        // The internally-tagged ReadinessProbe can't carry deny_unknown_fields,
        // so its payload is a FileProbe struct that does. A typo'd sibling
        // (here `idle_valeu`) -- which would otherwise silently disable the
        // quiet-period guard -- must be rejected, not dropped.
        let toml_str = r##"
[repos]

[tools.claude]
bin = "claude"

[tools.claude.readiness]
type = "file"
pattern = "~/r/{session_id}.json"
state_key = "state"
idle_value = "idle"
idle_valeu = "idle"
"##;
        let err = toml::from_str::<Config>(toml_str).unwrap_err().to_string();
        assert!(
            err.contains("idle_valeu") || err.contains("unknown field"),
            "expected the typo'd probe key to be rejected; got: {err}"
        );
    }

    #[test]
    fn session_discovery_rejects_unknown_key() {
        let toml_str = r##"
[repos]

[tools.claude]
bin = "claude"

[tools.claude.session_discovery]
type = "file"
pattern = "~/.claude/sessions/{pid}.json"
id_key = "sessionId"
bogus = "x"
"##;
        let err = toml::from_str::<Config>(toml_str).unwrap_err().to_string();
        assert!(
            err.contains("bogus") || err.contains("unknown field"),
            "expected the typo'd discovery key to be rejected; got: {err}"
        );
    }

    #[test]
    fn readiness_probe_unknown_type_still_rejected() {
        // The tag itself stays validated: an unknown `type` is a loud error.
        let toml_str = r##"
[repos]

[tools.claude]
bin = "claude"

[tools.claude.readiness]
type = "bogus"
"##;
        let err = toml::from_str::<Config>(toml_str).unwrap_err().to_string();
        assert!(
            err.contains("bogus") || err.contains("unknown variant"),
            "expected unknown-variant rejection; got: {err}"
        );
    }
}
