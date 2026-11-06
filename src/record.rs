//! Reads an install command back out of a shell history line.
//!
//! A scan can tell that a package appeared, but not how. The exact invocation
//! — `--HEAD`, a pinned `@v1.2.3`, which of two taps it came from — only exists
//! at the moment the command runs.

/// What a shell command did to the package inventory.
#[derive(Debug, PartialEq, Eq)]
pub struct Recorded {
    pub provider: &'static str,
    pub keys: Vec<String>,
    pub removal: bool,
}

pub fn parse(command: &str) -> Option<Recorded> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    // env assignments and `sudo` in front of the real command
    let start = tokens
        .iter()
        .position(|t| !t.contains('=') && *t != "sudo" && *t != "command")?;
    let tokens = &tokens[start..];
    let binary = tokens.first()?.rsplit('/').next()?;
    let rest = &tokens[1..];

    match binary {
        "brew" => brew(rest),
        "go" => simple(rest, &["install"], &[], "go").map(normalize_go),
        "cargo" => simple(rest, &["install"], &["uninstall"], "cargo"),
        "npm" | "pnpm" | "yarn" => npm(rest),
        "uv" => uv(rest),
        "mas" => simple(rest, &["install"], &["uninstall"], "mas"),
        "code" | "cursor" => code(rest),
        "claude" => claude(rest),
        "ollama" => simple(rest, &["pull"], &["rm", "remove"], "ollama")
            .map(|r| map_keys(r, |k| k.split(':').next().unwrap_or(&k).to_string())),
        _ => None,
    }
}

/// Collects the non-flag arguments of a subcommand.
fn operands(tokens: &[&str]) -> Vec<String> {
    tokens
        .iter()
        .filter(|t| !t.starts_with('-'))
        .map(|t| t.to_string())
        .collect()
}

fn simple(
    tokens: &[&str],
    add: &[&str],
    remove: &[&str],
    provider: &'static str,
) -> Option<Recorded> {
    let sub = *tokens.first()?;
    let removal = remove.contains(&sub);
    if !removal && !add.contains(&sub) {
        return None;
    }
    let keys = operands(&tokens[1..]);
    (!keys.is_empty()).then_some(Recorded {
        provider,
        keys,
        removal,
    })
}

fn map_keys(mut recorded: Recorded, f: impl Fn(String) -> String) -> Recorded {
    recorded.keys = recorded.keys.into_iter().map(f).collect();
    recorded
}

/// The manifest pins Go tools at `@latest`; a one-off `@v1.2.3` install is
/// still the same tool.
fn normalize_go(recorded: Recorded) -> Recorded {
    map_keys(recorded, |key| match key.split_once('@') {
        Some((path, _)) => format!("{path}@latest"),
        None => format!("{key}@latest"),
    })
}

fn brew(tokens: &[&str]) -> Option<Recorded> {
    let sub = *tokens.first()?;
    let args = &tokens[1..];
    let cask = args.iter().any(|t| *t == "--cask" || *t == "--casks");

    let (provider, removal) = match sub {
        "install" | "reinstall" => (if cask { "cask" } else { "brew" }, false),
        "uninstall" | "remove" | "rm" => (if cask { "cask" } else { "brew" }, true),
        "tap" => ("tap", false),
        "untap" => ("tap", true),
        _ => return None,
    };

    let keys = operands(args);
    (!keys.is_empty()).then_some(Recorded {
        provider,
        keys,
        removal,
    })
}

fn npm(tokens: &[&str]) -> Option<Recorded> {
    let sub = *tokens.first()?;
    let args = &tokens[1..];
    // only global installs land in the manifest
    if !args.iter().any(|t| *t == "-g" || *t == "--global") {
        return None;
    }
    let removal = matches!(sub, "uninstall" | "remove" | "rm" | "un");
    if !removal && !matches!(sub, "install" | "i" | "add") {
        return None;
    }
    let keys = operands(args)
        .into_iter()
        // `npm install -g` with no package reinstalls the tree
        .filter(|k| !k.is_empty())
        .collect::<Vec<_>>();
    (!keys.is_empty()).then_some(Recorded {
        provider: "npm",
        keys,
        removal,
    })
}

fn uv(tokens: &[&str]) -> Option<Recorded> {
    if *tokens.first()? != "tool" {
        return None;
    }
    simple(&tokens[1..], &["install"], &["uninstall"], "uv")
}

fn claude(tokens: &[&str]) -> Option<Recorded> {
    if *tokens.first()? != "plugin" {
        return None;
    }
    simple(
        &tokens[1..],
        &["install"],
        &["uninstall", "remove"],
        "claude",
    )
}

/// The editors take the extension id as the value of a flag, not as an operand.
fn code(tokens: &[&str]) -> Option<Recorded> {
    let mut keys = Vec::new();
    let mut removal = false;
    let mut iter = tokens.iter().peekable();
    while let Some(token) = iter.next() {
        match *token {
            "--install-extension" => {
                if let Some(id) = iter.next() {
                    keys.push(id.to_string());
                }
            }
            "--uninstall-extension" => {
                removal = true;
                if let Some(id) = iter.next() {
                    keys.push(id.to_string());
                }
            }
            _ => {}
        }
    }
    (!keys.is_empty()).then_some(Recorded {
        provider: "vscode",
        keys,
        removal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(command: &str) -> Vec<String> {
        parse(command).unwrap().keys
    }

    #[test]
    fn reads_a_formula_install() {
        let got = parse("brew install ripgrep fd").unwrap();
        assert_eq!(got.provider, "brew");
        assert_eq!(got.keys, vec!["ripgrep", "fd"]);
        assert!(!got.removal);
    }

    #[test]
    fn a_cask_install_is_not_a_formula_install() {
        assert_eq!(parse("brew install --cask arc").unwrap().provider, "cask");
    }

    #[test]
    fn flags_are_not_package_names() {
        assert_eq!(keys("brew install --HEAD --verbose neovim"), vec!["neovim"]);
    }

    #[test]
    fn uninstalling_is_recorded_as_a_removal() {
        assert!(parse("brew uninstall --cask arc").unwrap().removal);
        assert!(parse("brew untap someone/tap").unwrap().removal);
    }

    #[test]
    fn a_pinned_go_version_is_still_the_same_tool() {
        assert_eq!(
            keys("go install github.com/x/y/cmd/z@v1.2.3"),
            vec!["github.com/x/y/cmd/z@latest"]
        );
    }

    #[test]
    fn only_global_npm_installs_count() {
        assert!(parse("npm install left-pad").is_none());
        assert_eq!(keys("npm install -g prettier"), vec!["prettier"]);
    }

    #[test]
    fn reads_a_uv_tool_install() {
        let got = parse("uv tool install ruff").unwrap();
        assert_eq!(got.provider, "uv");
        assert_eq!(got.keys, vec!["ruff"]);
    }

    #[test]
    fn an_extension_id_comes_from_a_flag() {
        let got = parse("code --install-extension rust-lang.rust-analyzer").unwrap();
        assert_eq!(got.provider, "vscode");
        assert_eq!(got.keys, vec!["rust-lang.rust-analyzer"]);
    }

    #[test]
    fn an_ollama_tag_is_dropped_like_the_manifest_drops_it() {
        assert_eq!(keys("ollama pull llama3.2:latest"), vec!["llama3.2"]);
    }

    #[test]
    fn sudo_and_env_assignments_do_not_hide_the_command() {
        assert_eq!(
            keys("HOMEBREW_NO_AUTO_UPDATE=1 brew install jq"),
            vec!["jq"]
        );
        assert_eq!(keys("sudo mas install 497799835"), vec!["497799835"]);
    }

    #[test]
    fn ordinary_commands_are_ignored() {
        assert!(parse("git status").is_none());
        assert!(parse("brew list").is_none());
        assert!(parse("go build ./...").is_none());
    }
}
