use anyhow::Result;

use super::{Installed, Provider};
use crate::chezmoi::ChezmoiData;
use crate::util::{capture, has_bin, home};

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
        state_file().is_some_and(|p| p.is_file()) || has_bin("claude")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        let Some(path) = state_file() else {
            return Ok(Vec::new());
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Ok(Vec::new());
        };
        Ok(parse(&serde_json::from_str(&raw)?))
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.field_of_each(&["claude", "plugins", "install"], &["name", "marketplace"])
    }
}

fn state_file() -> Option<std::path::PathBuf> {
    Some(home().ok()?.join(".claude/plugins/installed_plugins.json"))
}

/// Reads the plugin state file rather than `claude plugin list`, which spends
/// a whole second starting Node to print what this file already holds.
///
/// Only user-scope plugins come from the manifest; a local- or project-scope
/// one belongs to whatever project it was enabled in, so each plugin is kept
/// only if one of its install records is user-scope.
fn parse(state: &serde_json::Value) -> Vec<Installed> {
    let Some(plugins) = state.get("plugins").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, records) in plugins {
        let Some(records) = records.as_array() else {
            continue;
        };
        let Some(user) = records
            .iter()
            .find(|r| r.get("scope").and_then(|s| s.as_str()) == Some("user"))
        else {
            continue;
        };
        let mut item = Installed::new(name.clone());
        item.version = user
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .filter(|v| v != "unknown");
        out.push(item);
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> serde_json::Value {
        serde_json::json!({
            "version": 2,
            "plugins": {
                "claude-dashboard@claude-dashboard": [
                    {"scope": "user", "version": "1.29.0"}
                ],
                "frontend-design@claude-plugins-official": [
                    {"scope": "local", "version": "unknown"},
                    {"scope": "project", "version": "unknown"}
                ]
            }
        })
    }

    #[test]
    fn reads_user_scope_plugins() {
        let got = parse(&sample());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "claude-dashboard@claude-dashboard");
        assert_eq!(got[0].version.as_deref(), Some("1.29.0"));
    }

    #[test]
    fn leaves_a_project_scoped_plugin_alone() {
        assert!(
            !parse(&sample())
                .iter()
                .any(|i| i.key.starts_with("frontend-design"))
        );
    }

    #[test]
    fn an_unknown_version_is_no_version() {
        let state = serde_json::json!({
            "plugins": {"x@y": [{"scope": "user", "version": "unknown"}]}
        });
        assert_eq!(parse(&state)[0].version, None);
    }
}
