use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_tool")]
    pub default_tool: String,
    pub verticals: HashMap<String, Vertical>,
    #[serde(default)]
    pub remotes: HashMap<String, Remote>,
    #[serde(default)]
    pub hooks: Hooks,
    #[serde(default)]
    pub harnesses: HashMap<String, HarnessConfig>,
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
pub struct Vertical {
    pub dir: String,
    pub color: String,
    /// Override default_tool for this vertical.
    #[serde(default)]
    pub tool: Option<String>,
    /// Create git worktrees for session isolation. Default: true for harness sessions.
    #[serde(default = "default_true")]
    pub worktree: bool,
    /// Effort level for the harness (e.g., "high", "max").
    #[serde(default)]
    pub effort: Option<String>,
    /// Permission mode for the harness (e.g., "auto", "plan").
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Max budget in USD per session.
    #[serde(default)]
    pub max_budget_usd: Option<f64>,
    /// Text appended to the Claude system prompt for this vertical.
    /// Multiple entries are joined with newlines.
    #[serde(default)]
    pub append_system_prompt: Option<Vec<String>>,
}

fn default_true() -> bool {
    true
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
pub struct HarnessConfig {
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
    /// External command for status display.
    #[serde(default)]
    pub status_command: Option<String>,
}

fn default_discovery_none() -> SessionDiscovery {
    SessionDiscovery::None
}

/// Reserved command names that cannot be used as harness names.
const RESERVED_NAMES: &[&str] = &[
    "init", "ls", "save", "restore", "new", "rename", "kill",
    "switch", "tmux-status", "claude-status", "completions",
];

impl HarnessConfig {
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
            status_command: Some("muxr claude-status".to_string()),
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
    pub fn restore_command(
        &self,
        session_name: Option<&str>,
        resume_id: Option<&str>,
    ) -> String {
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

    /// Build the launch command with vertical-specific settings.
    pub fn launch_command_with_vertical(
        &self,
        session_name: Option<&str>,
        resume_id: Option<&str>,
        model: Option<&str>,
        vertical: Option<&Vertical>,
    ) -> String {
        let mut cmd = self.launch_command(session_name, resume_id, model);

        if let Some(v) = vertical {
            if let Some(ref effort) = v.effort {
                cmd.push_str(&format!(" --effort {}", shell_escape(effort)));
            }
            if let Some(ref mode) = v.permission_mode {
                cmd.push_str(&format!(" --permission-mode {}", shell_escape(mode)));
            }
            if let Some(budget) = v.max_budget_usd {
                cmd.push_str(&format!(" --max-budget-usd {budget}"));
            }
            if let Some(ref prompts) = v.append_system_prompt {
                let joined = prompts.join("\n");
                cmd.push_str(&format!(" --append-system-prompt {}", shell_escape(&joined)));
            }
        }

        cmd
    }

    /// Build the rename command to send to the pane.
    pub fn build_rename_command(&self, name: &str) -> Option<String> {
        self.rename_command
            .as_ref()
            .map(|cmd| interpolate(cmd, "name", name))
    }
}

/// Interpolate a `{key}` placeholder with a shell-escaped value.
pub fn interpolate(template: &str, key: &str, value: &str) -> String {
    let placeholder = format!("{{{key}}}");
    if template.contains(&placeholder) {
        template.replace(&placeholder, &shell_escape(value))
    } else {
        template.to_string()
    }
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

        // Validate no name collisions between verticals, remotes, and harnesses
        for name in config.remotes.keys() {
            if config.verticals.contains_key(name) {
                anyhow::bail!(
                    "Name collision: '{name}' is defined as both a vertical and a remote"
                );
            }
        }
        for name in config.harnesses.keys() {
            if config.verticals.contains_key(name) {
                anyhow::bail!(
                    "Name collision: '{name}' is defined as both a vertical and a harness"
                );
            }
            if config.remotes.contains_key(name) {
                anyhow::bail!(
                    "Name collision: '{name}' is defined as both a remote and a harness"
                );
            }
            if RESERVED_NAMES.contains(&name.as_str()) {
                anyhow::bail!(
                    "Harness name '{name}' is reserved (conflicts with built-in command)"
                );
            }
        }

        Ok(config)
    }

    pub fn path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        let config_dir = home.join(".config").join("muxr");
        Ok(config_dir.join("config.toml"))
    }

    pub fn state_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        let config_dir = home.join(".config").join("muxr");
        Ok(config_dir.join("state.json"))
    }

    pub fn resolve_dir(&self, vertical: &str) -> Result<PathBuf> {
        let v = self
            .verticals
            .get(vertical)
            .with_context(|| format!("Unknown vertical: {vertical}"))?;
        let expanded = shellexpand::tilde(&v.dir);
        Ok(PathBuf::from(expanded.as_ref()))
    }

    /// All known names (verticals + remotes) for validation and completions.
    pub fn all_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .verticals
            .keys()
            .chain(self.remotes.keys())
            .map(|s| s.as_str())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Resolve which tool to use for a vertical.
    /// Priority: explicit override > vertical config > default_tool
    pub fn resolve_tool(&self, vertical: &str, tool_override: Option<&str>) -> String {
        if let Some(t) = tool_override {
            return t.to_string();
        }
        if let Some(v) = self.verticals.get(vertical)
            && let Some(ref t) = v.tool
        {
            return t.clone();
        }
        self.default_tool.clone()
    }

    /// Get the harness config for a tool name.
    /// Checks user config first, then falls back to built-in definitions.
    pub fn harness_for(&self, tool: &str) -> Option<HarnessConfig> {
        if let Some(h) = self.harnesses.get(tool) {
            return Some(h.clone());
        }
        // Built-in defaults
        if tool == "claude" {
            return Some(HarnessConfig::builtin_claude());
        }
        None
    }

    /// All configured harness names (explicit + built-in).
    pub fn harness_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.harnesses.keys().cloned().collect();
        // Add built-in claude if not overridden
        if !names.contains(&"claude".to_string()) {
            names.push("claude".to_string());
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
            eprintln!("  hook: {cmd}");
            let result = std::process::Command::new("sh")
                .args(["-c", cmd])
                .current_dir(dir)
                .env("PATH", &path)
                .status();
            match result {
                Ok(s) if !s.success() => eprintln!("  hook warning: {cmd} exited {s}"),
                Err(e) => eprintln!("  hook warning: {cmd} failed: {e}"),
                _ => {}
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
        self.verticals
            .get(name)
            .map(|v| v.color.as_str())
            .or_else(|| self.remotes.get(name).map(|r| r.color.as_str()))
            .unwrap_or("#8a7f83")
    }

    /// Generate a default config file with example verticals.
    pub fn default_template() -> String {
        r##"# muxr configuration
# Verticals define your project estates.
# Each vertical maps to a directory and a status bar color.

default_tool = "claude"

# [verticals.work]
# dir = "~/projects/work"
# color = "#7aa2f7"
# tool = "claude"    # optional, overrides default_tool
#
# [verticals.personal]
# dir = "~/projects/personal"
# color = "#9ece6a"

# [hooks]
# pre_create = ["mise install"]
# path = ["~/.local/share/mise/shims"]

# Harness definitions. Claude is built-in (zero config needed).
# Only define [harnesses.claude] to override the built-in defaults.
# Other harnesses must be configured explicitly.
#
# [harnesses.opencode]
# bin = "opencode"
# session_discovery = { type = "none" }
"##
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> Config {
        let toml_str = r##"
default_tool = "claude"

[verticals.work]
dir = "~/projects/work"
color = "#7aa2f7"
tool = "claude"

[verticals.personal]
dir = "~/projects/personal"
color = "#9ece6a"
tool = "opencode"

[remotes.lab]
project = "my-project"
zone = "us-central1-a"
user = "deploy"
color = "#d29922"

[harnesses.opencode]
bin = "opencode"
session_discovery = { type = "none" }
"##;
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn parse_valid_config() {
        let config = sample_config();
        assert_eq!(config.default_tool, "claude");
        assert_eq!(config.verticals.len(), 2);
        assert_eq!(config.remotes.len(), 1);
        assert_eq!(config.harnesses.len(), 1);
    }

    #[test]
    fn default_tool_is_claude() {
        let config: Config = toml::from_str("[verticals]").unwrap();
        assert_eq!(config.default_tool, "claude");
        assert!(config.harnesses.is_empty());
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
    fn color_for_vertical() {
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
    fn name_collision_vertical_remote_rejected() {
        let toml_str = r##"
[verticals.lab]
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
            .any(|name| config.verticals.contains_key(name));
        assert!(has_collision);
    }

    #[test]
    fn name_collision_harness_vertical_detected() {
        let toml_str = r##"
[verticals.opencode]
dir = "~/oc"
color = "#fff"

[harnesses.opencode]
bin = "opencode"
session_discovery = { type = "none" }
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let has_collision = config
            .harnesses
            .keys()
            .any(|name| config.verticals.contains_key(name));
        assert!(has_collision);
    }

    #[test]
    fn reserved_harness_name_detected() {
        assert!(RESERVED_NAMES.contains(&"save"));
        assert!(RESERVED_NAMES.contains(&"switch"));
        assert!(!RESERVED_NAMES.contains(&"claude"));
    }

    #[test]
    fn hooks_default_empty() {
        let config: Config = toml::from_str("[verticals]").unwrap();
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
        let h = HarnessConfig::builtin_claude();
        assert_eq!(h.bin, "claude");
        assert_eq!(h.rename_command, Some("/rename {name}".to_string()));
        assert!(matches!(h.session_discovery, SessionDiscovery::File { .. }));
    }

    #[test]
    fn harness_for_returns_builtin_claude() {
        let config: Config = toml::from_str("[verticals]").unwrap();
        let h = config.harness_for("claude").unwrap();
        assert_eq!(h.bin, "claude");
    }

    #[test]
    fn harness_for_returns_configured() {
        let config = sample_config();
        let h = config.harness_for("opencode").unwrap();
        assert_eq!(h.bin, "opencode");
    }

    #[test]
    fn harness_for_unknown_returns_none() {
        let config = sample_config();
        assert!(config.harness_for("cursor").is_none());
    }

    #[test]
    fn harness_config_overrides_builtin() {
        let toml_str = r##"
[verticals]

[harnesses.claude]
bin = "claude"
args = ["--name", "{name}", "--verbose"]
session_discovery = { type = "none" }
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        let h = config.harness_for("claude").unwrap();
        assert_eq!(h.args.len(), 3); // overridden, not the built-in 2
        assert!(matches!(h.session_discovery, SessionDiscovery::None));
    }

    #[test]
    fn launch_command_bare() {
        let h = HarnessConfig::builtin_claude();
        assert_eq!(h.launch_command(None, None, None), "claude");
    }

    #[test]
    fn launch_command_with_name() {
        let h = HarnessConfig::builtin_claude();
        let cmd = h.launch_command(Some("work/api"), None, None);
        assert_eq!(cmd, "claude --name 'work/api'");
    }

    #[test]
    fn launch_command_with_resume_and_model() {
        let h = HarnessConfig::builtin_claude();
        let cmd = h.launch_command(Some("tanuki/opus"), Some("abc-123"), Some("claude-opus-4-7"));
        assert_eq!(
            cmd,
            "claude --name 'tanuki/opus' --resume 'abc-123' --model 'claude-opus-4-7'"
        );
    }

    #[test]
    fn launch_command_shell_escapes_quotes() {
        let h = HarnessConfig::builtin_claude();
        let cmd = h.launch_command(Some("it's a test"), None, None);
        assert!(cmd.contains("'it'\\''s a test'"));
    }

    #[test]
    fn build_rename_command_interpolates() {
        let h = HarnessConfig::builtin_claude();
        let cmd = h.build_rename_command("tanuki/opus").unwrap();
        assert_eq!(cmd, "/rename 'tanuki/opus'");
    }

    #[test]
    fn build_rename_command_none_when_not_configured() {
        let h = HarnessConfig {
            rename_command: None,
            ..HarnessConfig::builtin_claude()
        };
        assert!(h.build_rename_command("test").is_none());
    }

    #[test]
    fn resolve_tool_flag_wins() {
        let config = sample_config();
        assert_eq!(config.resolve_tool("work", Some("cursor")), "cursor");
    }

    #[test]
    fn resolve_tool_vertical_config() {
        let config = sample_config();
        assert_eq!(config.resolve_tool("personal", None), "opencode");
    }

    #[test]
    fn resolve_tool_default_fallback() {
        let config = sample_config();
        // Unknown vertical falls back to default_tool
        assert_eq!(config.resolve_tool("nonexistent", None), "claude");
    }

    #[test]
    fn harness_names_includes_builtin() {
        let config: Config = toml::from_str("[verticals]").unwrap();
        let names = config.harness_names();
        assert!(names.contains(&"claude".to_string()));
    }

    #[test]
    fn harness_names_includes_configured() {
        let config = sample_config();
        let names = config.harness_names();
        assert!(names.contains(&"claude".to_string()));
        assert!(names.contains(&"opencode".to_string()));
    }

    #[test]
    fn hooks_parsed() {
        let toml_str = r##"
[verticals]

[hooks]
pre_create = ["mise install"]
path = ["~/.local/share/mise/shims"]
"##;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.hooks.pre_create, vec!["mise install"]);
        assert_eq!(config.hooks.path, vec!["~/.local/share/mise/shims"]);
    }
}
