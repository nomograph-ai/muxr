use crate::config::Config;
use anyhow::Result;
use clap::CommandFactory;
use std::collections::BTreeMap;
use std::fs;

/// Enumerate campaign slugs for each configured harness.
///
/// Returns a map of harness_name -> sorted campaign slugs. Used by
/// shell completions so `muxr <harness> <TAB>` suggests the campaigns
/// that actually exist on disk for that harness.
fn campaigns_by_harness(config: &Config) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for harness_name in config.harnesses.keys() {
        let Ok(dir) = config.resolve_dir(harness_name) else {
            continue;
        };
        let campaigns_dir = dir.join("campaigns");
        if !campaigns_dir.is_dir() {
            continue;
        }
        let mut slugs: Vec<String> = fs::read_dir(&campaigns_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| name != "TEMPLATE")
            .filter(|name| {
                campaigns_dir.join(name).join("campaign.md").is_file()
            })
            .collect();
        slugs.sort();
        out.insert(harness_name.clone(), slugs);
    }
    out
}

/// Derive subcommand names and descriptions from the Cli struct's clap metadata.
/// This eliminates hand-maintained command lists -- adding a new subcommand to
/// the Commands enum automatically includes it in completions.
fn derived_commands() -> Vec<(String, String)> {
    let cmd = crate::Cli::command();
    cmd.get_subcommands()
        .map(|sub| {
            let name = sub.get_name().to_string();
            let about = sub
                .get_about()
                .map(|a| a.to_string())
                .unwrap_or_default();
            (name, about)
        })
        .collect()
}

pub fn generate(shell: &str) -> Result<()> {
    match shell {
        "zsh" => generate_zsh(),
        "bash" => generate_bash(),
        "fish" => generate_fish(),
        _ => anyhow::bail!("Unsupported shell: {shell}. Use: zsh, bash, or fish"),
    }
}

fn generate_zsh() -> Result<()> {
    let commands = derived_commands();
    let config = Config::load().ok();
    let all_names = config
        .as_ref()
        .map(|c| {
            c.all_names()
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let tool_names = config
        .as_ref()
        .map(|c| c.tool_names())
        .unwrap_or_default();
    let campaigns = config
        .as_ref()
        .map(campaigns_by_harness)
        .unwrap_or_default();

    let harness_list = all_names.join(" ");
    let tool_list = tool_names.join(" ");

    let command_entries: String = commands
        .iter()
        .map(|(name, desc)| format!("        '{name}:{desc}'"))
        .collect::<Vec<_>>()
        .join("\n");

    // Emit a zsh array per harness: campaigns_<harness>=(slug1 slug2 ...)
    let campaigns_arrays: String = campaigns
        .iter()
        .map(|(h, slugs)| {
            let list = slugs.join(" ");
            format!("    campaigns_{h}=({list})")
        })
        .collect::<Vec<_>>()
        .join("\n");

    print!(
        r##"#compdef muxr

_muxr() {{
    local -a commands harnesses tools

    commands=(
{command_entries}
    )

    harnesses=({harness_list})
    tools=({tool_list})

{campaigns_arrays}

    if (( CURRENT == 2 )); then
        _alternative \
            'commands:command:compadd -a commands' \
            'harnesses:harness:compadd -a harnesses' \
            'tools:tool:compadd -a tools'
        return
    fi

    # If first arg is a tool, complete with tool subcommands
    if (( ${{+tools[(r)$words[2]]}} )); then
        compadd upgrade status compact model
        return
    fi

    # If first arg is a harness, complete with that harness's campaigns
    local harness=$words[2]
    if (( ${{+harnesses[(r)$harness]}} )); then
        local var="campaigns_${{harness}}"
        local -a slugs
        slugs=(${{(P)var}})
        if (( $#slugs )); then
            compadd -a slugs
        fi
    fi
}}

_muxr "$@"
"##
    );

    Ok(())
}

fn generate_bash() -> Result<()> {
    let commands = derived_commands();
    let config = Config::load().ok();
    let all_names = config
        .as_ref()
        .map(|c| {
            c.all_names()
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let tool_names = config
        .as_ref()
        .map(|c| c.tool_names())
        .unwrap_or_default();
    let campaigns = config
        .as_ref()
        .map(campaigns_by_harness)
        .unwrap_or_default();

    let harness_list = all_names.join(" ");
    let tool_list = tool_names.join(" ");
    let command_list: String = commands
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    print!(
        r#"_muxr_completions() {{
    local cur prev
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    prev="${{COMP_WORDS[COMP_CWORD-1]}}"

    local commands="{command_list}"
    local harnesses="{harness_list}"
    local tools="{tool_list}"

    if [[ $COMP_CWORD -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$commands $harnesses $tools" -- "$cur"))
        return
    fi

    # If first arg is a tool, offer tool subcommands
    case " $tools " in
        *" ${{COMP_WORDS[1]}} "*)
            COMPREPLY=($(compgen -W "upgrade status compact model" -- "$cur"))
            return
            ;;
    esac

    # If first arg is a harness, offer that harness's campaigns
    local harness="${{COMP_WORDS[1]}}"
    local campaigns=""
    case " $harnesses " in
        *" $harness "*)
"#
    );

    // Emit per-harness campaign assignments
    for (h, slugs) in &campaigns {
        let list = slugs.join(" ");
        println!(
            "            [[ \"$harness\" == \"{h}\" ]] && campaigns=\"{list}\""
        );
    }

    print!(
        r#"            COMPREPLY=($(compgen -W "$campaigns" -- "$cur"))
            ;;
    esac
}}

complete -F _muxr_completions muxr
"#
    );

    Ok(())
}

fn generate_fish() -> Result<()> {
    let commands = derived_commands();
    let config = Config::load().ok();
    let verticals = config
        .as_ref()
        .map(|c| {
            c.all_names()
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let harness_names = config
        .as_ref()
        .map(|c| c.tool_names())
        .unwrap_or_default();

    println!("# muxr fish completions");
    println!("complete -c muxr -f");
    println!();

    // Subcommands -- derived from clap
    for (name, desc) in &commands {
        println!("complete -c muxr -n '__fish_use_subcommand' -a '{name}' -d '{desc}'");
    }

    // Harnesses -- from runtime config
    for v in &verticals {
        println!("complete -c muxr -n '__fish_use_subcommand' -a '{v}' -d 'Open {v} session'");
    }

    // Harnesses -- from config + built-ins
    for h in &harness_names {
        println!("complete -c muxr -n '__fish_use_subcommand' -a '{h}' -d '{h} harness'");
        println!("complete -c muxr -n '__fish_seen_subcommand_from {h}' -a 'upgrade' -d 'Restart sessions'");
        println!("complete -c muxr -n '__fish_seen_subcommand_from {h}' -a 'status' -d 'Show status'");
    }

    Ok(())
}
