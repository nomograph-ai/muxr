use crate::config::Config;
use crate::tmux::Tmux;
use anyhow::Result;
use clap::CommandFactory;

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
    let all_names = Config::load()
        .map(|c| {
            c.all_names()
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let sessions = Tmux::new(None).list_sessions().unwrap_or_default();

    let vertical_list = all_names.join(" ");
    let session_list: String = sessions
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    let command_entries: String = commands
        .iter()
        .map(|(name, desc)| format!("        '{name}:{desc}'"))
        .collect::<Vec<_>>()
        .join("\n");

    print!(
        r##"#compdef muxr

_muxr() {{
    local -a commands verticals sessions

    commands=(
{command_entries}
    )

    verticals=({vertical_list})
    sessions=({session_list})

    if (( CURRENT == 2 )); then
        _alternative \
            'commands:command:compadd -a commands' \
            'verticals:vertical:compadd -a verticals'
        return
    fi

    # If first arg is a vertical, complete with active session contexts
    local vertical=$words[2]
    if (( ${{+verticals[(r)$vertical]}} )); then
        # Offer existing session contexts for this vertical
        local -a contexts
        for s in $sessions; do
            if [[ "$s" == "$vertical/"* ]]; then
                contexts+=("${{s#$vertical/}}")
            fi
        done
        if (( $#contexts )); then
            compadd -a contexts
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
    let verticals = Config::load()
        .map(|c| {
            c.all_names()
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let sessions = Tmux::new(None).list_sessions().unwrap_or_default();

    let vertical_list = verticals.join(" ");
    let command_list: String = commands
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    print!(
        r#"_muxr_completions() {{
    local cur prev commands verticals
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    prev="${{COMP_WORDS[COMP_CWORD-1]}}"

    commands="{command_list}"
    verticals="{vertical_list}"

    if [[ $COMP_CWORD -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$commands $verticals" -- "$cur"))
        return
    fi

    # If first arg is a vertical, complete with session contexts
    local vertical="${{COMP_WORDS[1]}}"
    case " $verticals " in
        *" $vertical "*)
            local contexts=""
"#
    );

    for (name, _) in &sessions {
        if let Some((_vert, ctx)) = name.split_once('/') {
            let vert = name.split('/').next().unwrap_or("");
            println!(
                "            [[ \"$vertical\" == \"{vert}\" ]] && contexts=\"$contexts {ctx}\""
            );
        }
    }

    print!(
        r#"            COMPREPLY=($(compgen -W "$contexts" -- "$cur"))
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
    let verticals = Config::load()
        .map(|c| {
            c.all_names()
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    println!("# muxr fish completions");
    println!("complete -c muxr -f");
    println!();

    // Subcommands -- derived from clap
    for (name, desc) in &commands {
        println!("complete -c muxr -n '__fish_use_subcommand' -a '{name}' -d '{desc}'");
    }

    // Verticals -- from runtime config
    for v in &verticals {
        println!("complete -c muxr -n '__fish_use_subcommand' -a '{v}' -d 'Open {v} session'");
    }

    Ok(())
}
