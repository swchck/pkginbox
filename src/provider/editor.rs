use anyhow::Result;

use super::{Installed, Provider};
use crate::chezmoi::ChezmoiData;
use crate::util::{capture, has_bin};

pub struct VsCode;
pub struct ClaudePlugins;

impl Provider for VsCode {
    fn id(&self) -> &'static str {
        "vscode"
    }

    fn available(&self) -> bool {
        has_bin("code")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        let raw = capture("code", &["--list-extensions"])?;
        Ok(raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            // the marketplace is case-insensitive about publisher ids, and the
            // manifest and the CLI do not always agree on the casing
            .map(|id| Installed::new(id).with_alias(id.to_lowercase()))
            .collect())
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        let declared = data.list(&["vscode", "extensions"]);
        let lowered: Vec<String> = declared.iter().map(|id| id.to_lowercase()).collect();
        declared.into_iter().chain(lowered).collect()
    }
}

impl Provider for ClaudePlugins {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn available(&self) -> bool {
        has_bin("claude")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        Ok(parse(&capture("claude", &["plugin", "list"])?))
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.field_of_each(&["claude", "plugins", "install"], &["name", "marketplace"])
    }
}

/// `claude plugin list` prints `❯ name@marketplace` followed by indented
/// Version/Scope/Status lines. Only user-scope plugins come from the manifest;
/// a local-scope one belongs to whatever project it was enabled in.
fn parse(raw: &str) -> Vec<Installed> {
    let mut out: Vec<Installed> = Vec::new();
    let mut pending: Option<(String, Option<String>)> = None;
    let mut user_scope = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("❯ ") {
            push(&mut out, pending.take(), user_scope);
            pending = Some((name.trim().to_string(), None));
            user_scope = false;
        } else if let Some(version) = trimmed.strip_prefix("Version:")
            && let Some((_, slot)) = pending.as_mut()
        {
            *slot = Some(version.trim().to_string()).filter(|v| v != "unknown");
        } else if let Some(scope) = trimmed.strip_prefix("Scope:") {
            user_scope = scope.trim() == "user";
        }
    }
    push(&mut out, pending, user_scope);
    out
}

fn push(out: &mut Vec<Installed>, entry: Option<(String, Option<String>)>, user_scope: bool) {
    let Some((name, version)) = entry else { return };
    if !user_scope {
        return;
    }
    let mut item = Installed::new(name);
    item.version = version;
    out.push(item);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Installed plugins:\n\n  ❯ claude-dashboard@claude-dashboard\n    Version: 1.29.0\n    Scope: user\n    Status: ✔ enabled\n\n  ❯ frontend-design@claude-plugins-official\n    Version: unknown\n    Scope: local\n    Status: ✔ enabled\n";

    #[test]
    fn reads_user_scope_plugins() {
        let got = parse(SAMPLE);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "claude-dashboard@claude-dashboard");
        assert_eq!(got[0].version.as_deref(), Some("1.29.0"));
    }

    #[test]
    fn leaves_a_project_scoped_plugin_alone() {
        assert!(
            !parse(SAMPLE)
                .iter()
                .any(|i| i.key.starts_with("frontend-design"))
        );
    }
}
