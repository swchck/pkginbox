//! Reinstalling a package the manifest declares but the machine has lost.

/// The command that puts a declared package back, or `None` when the manifest
/// cannot install this kind of thing at all.
pub fn command(provider: &str, key: &str) -> Option<(&'static str, Vec<String>)> {
    let own = |bin: &'static str, args: &[&str]| {
        Some((bin, args.iter().map(|a| a.to_string()).collect::<Vec<_>>()))
    };
    match provider {
        "brew" => own("brew", &["install", key]),
        "cask" => own("brew", &["install", "--cask", key]),
        "tap" => own("brew", &["tap", key]),
        "mas" => own("mas", &["install", key]),
        "go" => own("go", &["install", key]),
        "cargo" => own("cargo", &["install", key]),
        "npm" => own("npm", &["install", "-g", key]),
        "uv" => own("uv", &["tool", "install", key]),
        "vscode" => own("code", &["--install-extension", key]),
        "claude" => own("claude", &["plugin", "install", key]),
        "ollama" => own("ollama", &["pull", key]),
        _ => None,
    }
}

/// How the command reads, for a confirmation prompt.
pub fn describe(provider: &str, key: &str) -> Option<String> {
    let (bin, args) = command(provider, key)?;
    Some(format!("{bin} {}", args.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cask_is_not_reinstalled_as_a_formula() {
        assert_eq!(describe("cask", "arc").unwrap(), "brew install --cask arc");
    }

    #[test]
    fn a_hand_installed_app_has_no_command() {
        assert!(command("app", "WezTerm.app").is_none());
    }
}
